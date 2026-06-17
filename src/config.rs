//! Runtime configuration, sourced from environment variables.

/// Parse a boolean environment variable leniently, returning `default` when the
/// variable is unset. Recognises `false`/`0`/`no`/`off` (and their `true`
/// counterparts) case-insensitively and ignores surrounding whitespace, so a
/// stray `COOKIE_SECURE=False` doesn't silently keep the opposite value.
fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "false" | "0" | "no" | "off" => false,
            "true" | "1" | "yes" | "on" => true,
            // An unrecognised value falls back to the default rather than
            // guessing — paired with a warning so a typo is visible.
            other => {
                tracing::warn!("{name}: unrecognised boolean value {other:?}; using {default}");
                default
            }
        },
        Err(_) => default,
    }
}

/// Application configuration resolved once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite connection string, e.g. `sqlite:golf.db`.
    pub database_url: String,
    /// TCP port to listen on.
    pub port: u16,
    /// When true (the default), the scheduler simulates bookings instead of
    /// hitting the club. Set `DRY_RUN=false` to make real bookings.
    pub dry_run: bool,
    /// Whether the session cookie carries the `Secure` attribute. True by
    /// default (deployed behind TLS); set `COOKIE_SECURE=false` only for local
    /// plain-HTTP development, where a `Secure` cookie would never be sent.
    pub cookie_secure: bool,
    /// Public base URL used to build links in emails (e.g. the `ts.net` URL).
    pub base_url: String,
    /// SMTP settings for outbound email; `None` disables email features.
    pub smtp: Option<SmtpConfig>,
}

/// Outbound SMTP configuration (Fastmail by default).
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// `From:` address (typically your Fastmail address).
    pub from: String,
}

// Manual Debug so SMTP credentials never leak (e.g. via `Config`'s derive).
impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &"[redacted]")
            .field("password", &"[redacted]")
            .field("from", &self.from)
            .finish()
    }
}

impl SmtpConfig {
    /// Build from env if the required vars are present. Host/port default to
    /// Fastmail's submission endpoint.
    fn from_env() -> Option<Self> {
        let (username, password, from) = (
            std::env::var("SMTP_USERNAME").ok()?,
            std::env::var("SMTP_PASSWORD").ok()?,
            std::env::var("SMTP_FROM").ok()?,
        );
        let host = std::env::var("SMTP_HOST").unwrap_or_else(|_| "smtp.fastmail.com".to_string());
        let port = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(465);
        Some(Self {
            host,
            port,
            username,
            password,
            from,
        })
    }
}

impl Config {
    pub fn from_env() -> Self {
        let database_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:golf.db".to_string());

        let port = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);

        // Dry-run unless explicitly disabled, so real bookings are always opt-in.
        let dry_run = env_bool("DRY_RUN", true);

        // Secure by default; only disabled explicitly for local HTTP dev.
        let cookie_secure = env_bool("COOKIE_SECURE", true);

        let base_url = std::env::var("APP_BASE_URL")
            .unwrap_or_else(|_| format!("http://localhost:{port}"))
            .trim_end_matches('/')
            .to_string();

        Self {
            database_url,
            port,
            dry_run,
            cookie_secure,
            base_url,
            smtp: SmtpConfig::from_env(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::env_bool;

    // Each test uses a unique var name so the shared process environment can't
    // race across parallel tests.
    #[test]
    fn unset_uses_default() {
        assert!(env_bool("CFG_TEST_UNSET", true));
        assert!(!env_bool("CFG_TEST_UNSET", false));
    }

    #[test]
    fn recognises_falsey_values_case_insensitively() {
        for v in ["false", "False", "FALSE", "0", "no", "NO", "off", " off "] {
            std::env::set_var("CFG_TEST_FALSEY", v);
            assert!(
                !env_bool("CFG_TEST_FALSEY", true),
                "expected false for {v:?}"
            );
        }
        std::env::remove_var("CFG_TEST_FALSEY");
    }

    #[test]
    fn recognises_truthy_values_case_insensitively() {
        for v in ["true", "True", "1", "yes", "on", " ON "] {
            std::env::set_var("CFG_TEST_TRUTHY", v);
            assert!(
                env_bool("CFG_TEST_TRUTHY", false),
                "expected true for {v:?}"
            );
        }
        std::env::remove_var("CFG_TEST_TRUTHY");
    }

    #[test]
    fn unrecognised_value_falls_back_to_default() {
        std::env::set_var("CFG_TEST_GARBAGE", "maybe");
        assert!(env_bool("CFG_TEST_GARBAGE", true));
        assert!(!env_bool("CFG_TEST_GARBAGE", false));
        std::env::remove_var("CFG_TEST_GARBAGE");
    }
}
