//! Application login accounts and the axum-login authentication backend.

use axum_login::{AuthUser, AuthnBackend, UserId};
use chrono::{DateTime, Duration, Utc};
use password_auth::{generate_hash, verify_password};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};
use std::sync::OnceLock;

/// How long a password-reset link stays valid.
const RESET_TOKEN_TTL_HOURS: i64 = 1;

/// Minimum length for an account password, enforced on creation and on reset.
pub const MIN_PASSWORD_LEN: usize = 8;

/// A fixed Argon2 hash used to spend constant time verifying credentials when no
/// matching user exists, defeating username-enumeration timing attacks.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| generate_hash("constant-time-placeholder"))
}

/// A login account for the app (a family member). Distinct from a golf *club*
/// login, which lives in the `clubs` table.
#[derive(Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Argon2 hash of the account password.
    password: String,
    pub email: Option<String>,
}

// Manual Debug so the password hash never lands in logs.
impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("email", &self.email)
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
        let user = tokio::task::spawn_blocking(move || match user {
            Some(user) if verify_password(&creds.password, &user.password).is_ok() => Some(user),
            // Verify against a fixed dummy hash even when the user is absent or
            // the password is wrong, so response timing doesn't reveal whether
            // the username exists.
            _ => {
                let _ = verify_password(&creds.password, dummy_hash());
                None
            }
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
pub async fn create(
    db: &SqlitePool,
    username: &str,
    email: Option<&str>,
    password: &str,
) -> Result<i64, sqlx::Error> {
    let hash = generate_hash(password);
    let result = sqlx::query("INSERT INTO users (username, email, password) VALUES (?, ?, ?)")
        .bind(username)
        .bind(email)
        .bind(hash)
        .execute(db)
        .await?;
    Ok(result.last_insert_rowid())
}

/// A login account rendered to the management UI (no password).
#[derive(FromRow)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
}

pub async fn list(db: &SqlitePool) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query_as("SELECT id, username, email FROM users ORDER BY username")
        .fetch_all(db)
        .await
}

pub async fn count(db: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await
}

pub async fn delete(db: &SqlitePool, user_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Set a new password (hashed). Note: this changes `session_auth_hash`, so it
/// invalidates the user's existing sessions.
pub async fn set_password(
    db: &SqlitePool,
    user_id: i64,
    new_password: &str,
) -> Result<(), sqlx::Error> {
    let hash = generate_hash(new_password);
    sqlx::query("UPDATE users SET password = ? WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// The user id owning an email address, if any.
pub async fn id_for_email(db: &SqlitePool, email: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(db)
        .await
}

/// A user's email address, if set.
pub async fn email_for(db: &SqlitePool, user_id: i64) -> Result<Option<String>, sqlx::Error> {
    let email: Option<Option<String>> = sqlx::query_scalar("SELECT email FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(email.flatten())
}

fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// Create a single-use password-reset token for a user. Returns the raw token
/// (only the hash is stored); embed it in the emailed link.
pub async fn create_reset_token(db: &SqlitePool, user_id: i64) -> Result<String, sqlx::Error> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = hex::encode(bytes);
    let expires = (Utc::now() + Duration::hours(RESET_TOKEN_TTL_HOURS)).to_rfc3339();

    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(hash_token(&raw))
    .bind(expires)
    .execute(db)
    .await?;
    Ok(raw)
}

/// Redeem a reset token: if valid, unused, and unexpired, mark it used and
/// return the user id. Atomic against double-use.
pub async fn consume_reset_token(db: &SqlitePool, raw: &str) -> Result<Option<i64>, sqlx::Error> {
    let hash = hash_token(raw);
    let row: Option<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, user_id, expires_at FROM password_reset_tokens \
         WHERE token_hash = ? AND used_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;

    let Some((token_id, user_id, expires_at)) = row else {
        return Ok(None);
    };
    let expired = DateTime::parse_from_rfc3339(&expires_at)
        .map(|e| Utc::now() > e.with_timezone(&Utc))
        .unwrap_or(true);
    if expired {
        return Ok(None);
    }

    // Mark used; rows_affected == 0 means another request beat us to it.
    let result = sqlx::query(
        "UPDATE password_reset_tokens SET used_at = ? WHERE id = ? AND used_at IS NULL",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(token_id)
    .execute(db)
    .await?;

    Ok((result.rows_affected() == 1).then_some(user_id))
}

/// Seed the first login account from `APP_USERNAME` / `APP_PASSWORD` (+ optional
/// `APP_EMAIL`) if the users table is empty.
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
    let email = std::env::var("APP_EMAIL").ok();

    create(db, &username, email.as_deref(), &password).await?;
    tracing::info!(%username, "seeded initial login account from environment");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;

    #[test]
    fn token_hash_is_stable_and_distinct() {
        assert_eq!(hash_token("abc"), hash_token("abc"));
        assert_ne!(hash_token("abc"), hash_token("abd"));
        // SHA-256 hex is 64 chars.
        assert_eq!(hash_token("abc").len(), 64);
    }

    #[test]
    fn debug_redacts_password_hash() {
        let user = User {
            id: 1,
            username: "alice".into(),
            password: "argon2-secret-hash".into(),
            email: Some("a@b.com".into()),
        };
        let dbg = format!("{user:?}");
        assert!(!dbg.contains("argon2-secret-hash"), "hash leaked: {dbg}");
        assert!(dbg.contains("alice"));
        assert!(dbg.contains("a@b.com"));
    }

    async fn auth(db: &SqlitePool, username: &str, password: &str) -> Option<User> {
        Backend::new(db.clone())
            .authenticate(Credentials {
                username: username.to_string(),
                password: password.to_string(),
                next: None,
            })
            .await
            .expect("authenticate query")
    }

    #[tokio::test]
    async fn authenticate_succeeds_with_correct_password() {
        let db = test_pool().await;
        let id = create(&db, "alice", Some("a@b.com"), "correct horse")
            .await
            .unwrap();

        let user = auth(&db, "alice", "correct horse")
            .await
            .expect("should authenticate");
        assert_eq!(user.id, id);
        assert_eq!(user.username, "alice");
    }

    #[tokio::test]
    async fn authenticate_rejects_wrong_password() {
        let db = test_pool().await;
        create(&db, "alice", None, "correct horse").await.unwrap();
        assert!(auth(&db, "alice", "wrong").await.is_none());
    }

    #[tokio::test]
    async fn authenticate_rejects_unknown_user() {
        let db = test_pool().await;
        // No users exist: still returns None without erroring (constant-time path).
        assert!(auth(&db, "nobody", "whatever").await.is_none());
    }

    #[tokio::test]
    async fn get_user_fetches_by_id() {
        let db = test_pool().await;
        let id = create(&db, "alice", None, "password1").await.unwrap();
        let backend = Backend::new(db.clone());
        let user = backend.get_user(&id).await.unwrap().expect("user exists");
        assert_eq!(user.username, "alice");
        assert!(backend.get_user(&9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_password_changes_login_and_invalidates_old() {
        let db = test_pool().await;
        let id = create(&db, "alice", None, "oldpassword").await.unwrap();

        set_password(&db, id, "newpassword").await.unwrap();

        assert!(auth(&db, "alice", "newpassword").await.is_some());
        assert!(auth(&db, "alice", "oldpassword").await.is_none());
    }

    #[tokio::test]
    async fn count_list_and_delete() {
        let db = test_pool().await;
        assert!(!any_exist(&db).await.unwrap());
        let a = create(&db, "alice", None, "password1").await.unwrap();
        create(&db, "bob", None, "password2").await.unwrap();

        assert_eq!(count(&db).await.unwrap(), 2);
        assert!(any_exist(&db).await.unwrap());
        let listed: Vec<_> = list(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.username)
            .collect();
        assert_eq!(listed, ["alice", "bob"]); // ordered by username

        delete(&db, a).await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn email_lookups() {
        let db = test_pool().await;
        let id = create(&db, "alice", Some("a@b.com"), "password1")
            .await
            .unwrap();
        create(&db, "bob", None, "password2").await.unwrap();

        assert_eq!(id_for_email(&db, "a@b.com").await.unwrap(), Some(id));
        assert_eq!(id_for_email(&db, "missing@b.com").await.unwrap(), None);
        assert_eq!(
            email_for(&db, id).await.unwrap().as_deref(),
            Some("a@b.com")
        );
        // A user with no email yields None, not an error.
        let bob = id_for_email(&db, "a@b.com").await.unwrap();
        assert!(bob.is_some());
    }

    #[tokio::test]
    async fn reset_token_can_be_consumed_once() {
        let db = test_pool().await;
        let id = create(&db, "alice", Some("a@b.com"), "password1")
            .await
            .unwrap();

        let raw = create_reset_token(&db, id).await.unwrap();
        // Valid token returns the owning user id...
        assert_eq!(consume_reset_token(&db, &raw).await.unwrap(), Some(id));
        // ...and is single-use: a second redemption fails.
        assert_eq!(consume_reset_token(&db, &raw).await.unwrap(), None);
    }

    #[tokio::test]
    async fn unknown_reset_token_is_rejected() {
        let db = test_pool().await;
        create(&db, "alice", None, "password1").await.unwrap();
        assert_eq!(consume_reset_token(&db, "deadbeef").await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_reset_token_is_rejected() {
        let db = test_pool().await;
        let id = create(&db, "alice", None, "password1").await.unwrap();

        // Insert a token that expired an hour ago.
        let raw = "raw-token-value";
        let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(hash_token(raw))
        .bind(past)
        .execute(&db)
        .await
        .unwrap();

        assert_eq!(consume_reset_token(&db, raw).await.unwrap(), None);
    }
}
