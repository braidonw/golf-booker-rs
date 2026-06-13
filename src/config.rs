//! Runtime configuration, sourced from environment variables.

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
        let dry_run = std::env::var("DRY_RUN")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        // Secure by default; only disabled explicitly for local HTTP dev.
        let cookie_secure = std::env::var("COOKIE_SECURE")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        Self {
            database_url,
            port,
            dry_run,
            cookie_secure,
        }
    }
}
