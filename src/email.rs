//! Outbound email via SMTP (Fastmail by default).
//!
//! The mailer is *optional*: with no SMTP config the app still runs and callers
//! get `Ok(false)` from [`Mailer::send`], so they can fall back (e.g. log a
//! password-reset link instead of mailing it).

use crate::config::SmtpConfig;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Clone)]
pub struct Mailer {
    inner: Option<Inner>,
}

#[derive(Clone)]
struct Inner {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    /// Build from SMTP config; `None` config yields a disabled mailer.
    pub fn from_config(smtp: Option<&SmtpConfig>) -> anyhow::Result<Self> {
        let Some(cfg) = smtp else {
            tracing::warn!(
                "SMTP not configured — email disabled (password-reset links will be logged)"
            );
            return Ok(Self { inner: None });
        };

        let from: Mailbox = cfg
            .from
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid SMTP_FROM '{}': {e}", cfg.from))?;
        let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());

        // Implicit TLS on 465; STARTTLS otherwise (e.g. 587).
        let builder = if cfg.port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?
        };
        // Bound a send so a hung SMTP server can't tie up the calling task
        // (notifications run inside scheduler job tasks) indefinitely.
        let transport = builder
            .port(cfg.port)
            .credentials(creds)
            .timeout(Some(std::time::Duration::from_secs(15)))
            .build();

        tracing::info!(host = %cfg.host, port = cfg.port, "SMTP configured");
        Ok(Self {
            inner: Some(Inner { transport, from }),
        })
    }

    /// Send a plain-text email. Returns `Ok(false)` if the mailer is disabled.
    pub async fn send(&self, to: &str, subject: &str, body: String) -> anyhow::Result<bool> {
        let Some(inner) = &self.inner else {
            return Ok(false);
        };
        let to: Mailbox = to
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid recipient '{to}': {e}"))?;
        let email = Message::builder()
            .from(inner.from.clone())
            .to(to)
            .subject(subject)
            .body(body)?;
        inner.transport.send(email).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smtp_config(from: &str) -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 465,
            username: "secret-login".to_string(),
            password: "hunter2-secret".to_string(),
            from: from.to_string(),
        }
    }

    #[test]
    fn no_config_yields_disabled_mailer() {
        let mailer = Mailer::from_config(None).expect("disabled mailer builds");
        assert!(mailer.inner.is_none());
    }

    #[tokio::test]
    async fn disabled_mailer_send_is_ok_false_not_error() {
        // Both callers rely on "disabled" being Ok(false), distinct from a send
        // failure — it lets them fall back rather than surface an error.
        let mailer = Mailer::from_config(None).unwrap();
        let sent = mailer
            .send("a@b.com", "hi", "body".to_string())
            .await
            .expect("disabled send must not error");
        assert!(!sent);
    }

    #[test]
    fn invalid_from_address_fails_at_build() {
        let err = match Mailer::from_config(Some(&smtp_config("not-an-address"))) {
            Ok(_) => panic!("a malformed From address should fail to build"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("invalid SMTP_FROM"), "got: {err}");
    }

    #[test]
    fn debug_redacts_smtp_credentials() {
        let dbg = format!("{:?}", smtp_config("me@example.com"));
        assert!(!dbg.contains("hunter2-secret"), "password leaked: {dbg}");
        assert!(!dbg.contains("secret-login"), "username leaked: {dbg}");
        assert!(
            dbg.contains("me@example.com"),
            "from should be shown: {dbg}"
        );
    }
}
