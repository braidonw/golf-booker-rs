//! Application login accounts and the axum-login authentication backend.

use axum_login::{AuthUser, AuthnBackend, UserId};
use password_auth::{generate_hash, verify_password};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};

/// A login account for the app (a family member). Distinct from a golf *club*
/// login, which lives in the `clubs` table.
#[derive(Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Argon2 hash of the account password.
    password: String,
}

// Manual Debug so the password hash never lands in logs.
impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        // Using the password hash means changing the password invalidates
        // existing sessions.
        self.password.as_bytes()
    }
}

/// Login form fields.
#[derive(Clone, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// Optional post-login redirect target (the page the user was sent from).
    pub next: Option<String>,
}

#[derive(Clone)]
pub struct Backend {
    db: SqlitePool,
}

impl Backend {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = sqlx::Error;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE username = ?")
            .bind(&creds.username)
            .fetch_optional(&self.db)
            .await?;

        // Argon2 verification is CPU-bound, so keep it off the async runtime.
        let user = tokio::task::spawn_blocking(move || {
            user.filter(|user| verify_password(&creds.password, &user.password).is_ok())
        })
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        Ok(user)
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await
    }
}

/// Convenience alias with our concrete backend baked in.
pub type AuthSession = axum_login::AuthSession<Backend>;

/// Whether any login account exists yet.
pub async fn any_exist(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await?;
    Ok(count > 0)
}

/// Create a login account with the given plaintext password (hashed before
/// storage). Returns the new user's id.
pub async fn create(db: &SqlitePool, username: &str, password: &str) -> Result<i64, sqlx::Error> {
    let hash = generate_hash(password);
    let result = sqlx::query("INSERT INTO users (username, password) VALUES (?, ?)")
        .bind(username)
        .bind(hash)
        .execute(db)
        .await?;
    Ok(result.last_insert_rowid())
}

/// Seed the first login account from `APP_USERNAME` / `APP_PASSWORD` if the
/// users table is empty. No-op if either var is missing or users already exist.
pub async fn seed_from_environment(db: &SqlitePool) -> anyhow::Result<()> {
    if any_exist(db).await? {
        return Ok(());
    }

    let (Ok(username), Ok(password)) =
        (std::env::var("APP_USERNAME"), std::env::var("APP_PASSWORD"))
    else {
        tracing::warn!(
            "no login accounts and APP_USERNAME/APP_PASSWORD not set — set them to seed the first account"
        );
        return Ok(());
    };

    create(db, &username, &password).await?;
    tracing::info!(%username, "seeded initial login account from environment");
    Ok(())
}
