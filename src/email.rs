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
        let transport = builder.port(cfg.port).credentials(creds).build();

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
