//! Golf clubs we can book against, and their stored credentials.
//!
//! One operator (single household), so club logins are stored centrally. The
//! `password` is plaintext at rest because it has to be replayed to the club on
//! login — treat the whole row as a secret and never render credentials back.

use sqlx::{FromRow, SqlitePool};

#[derive(Clone, FromRow)]
pub struct Club {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub member_id: String,
    /// IANA timezone the club books in (e.g. `Australia/Sydney`). Tee sheets
    /// open at local time, so scheduling is interpreted/displayed in this zone.
    pub timezone: String,
}

// Manual Debug so credentials never end up in logs.
impl std::fmt::Debug for Club {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Club")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("username", &"[redacted]")
            .field("password", &"[redacted]")
            .field("member_id", &"[redacted]")
            .field("timezone", &self.timezone)
            .finish()
    }
}

const COLUMNS: &str = "id, name, base_url, username, password, member_id, timezone";

pub async fn list(db: &SqlitePool) -> Result<Vec<Club>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM clubs ORDER BY name"))
        .fetch_all(db)
        .await
}

pub async fn get(db: &SqlitePool, id: i64) -> Result<Option<Club>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM clubs WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create(
    db: &SqlitePool,
    name: &str,
    base_url: &str,
    username: &str,
    password: &str,
    member_id: &str,
    timezone: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO clubs (name, base_url, username, password, member_id, timezone) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(base_url)
    .bind(username)
    .bind(password)
    .bind(member_id)
    .bind(timezone)
    .execute(db)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Update a club. `password` is optional: `None` leaves the stored password
/// unchanged, so the edit form needn't re-enter it.
pub async fn update(
    db: &SqlitePool,
    id: i64,
    name: &str,
    base_url: &str,
    username: &str,
    password: Option<&str>,
    member_id: &str,
    timezone: &str,
) -> Result<(), sqlx::Error> {
    match password {
        Some(password) => {
            sqlx::query(
                "UPDATE clubs SET name = ?, base_url = ?, username = ?, password = ?, \
                 member_id = ?, timezone = ? WHERE id = ?",
            )
            .bind(name)
            .bind(base_url)
            .bind(username)
            .bind(password)
            .bind(member_id)
            .bind(timezone)
            .bind(id)
            .execute(db)
            .await?;
        }
        None => {
            sqlx::query(
                "UPDATE clubs SET name = ?, base_url = ?, username = ?, member_id = ?, \
                 timezone = ? WHERE id = ?",
            )
            .bind(name)
            .bind(base_url)
            .bind(username)
            .bind(member_id)
            .bind(timezone)
            .bind(id)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM clubs WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

async fn any_exist(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clubs")
        .fetch_one(db)
        .await?;
    Ok(count > 0)
}

/// Seed clubs from the environment if the table is empty, so an existing
/// single-/dual-club setup migrates transparently. Each club is optional and
/// only seeded if all of its required vars are present.
///
/// - The Ridge:  `RIDGE_BASE_URL`, `RIDGE_USERNAME`, `RIDGE_PASSWORD`, `RIDGE_MEMBER_ID`
/// - NSW Golf:   `NSW_BASE_URL`,   `NSW_USERNAME`,   `NSW_PASSWORD`,   `NSW_MEMBER_ID`
///
/// `<PREFIX>_TIMEZONE` overrides the default `Australia/Sydney`.
pub async fn seed_from_environment(db: &SqlitePool) -> anyhow::Result<()> {
    if any_exist(db).await? {
        return Ok(());
    }

    for (name, prefix) in [("The Ridge", "RIDGE"), ("NSW Golf Club", "NSW")] {
        let var = |suffix: &str| std::env::var(format!("{prefix}_{suffix}"));
        let (Ok(base_url), Ok(username), Ok(password), Ok(member_id)) = (
            var("BASE_URL"),
            var("USERNAME"),
            var("PASSWORD"),
            var("MEMBER_ID"),
        ) else {
            continue;
        };
        let timezone = var("TIMEZONE").unwrap_or_else(|_| "Australia/Sydney".to_string());

        create(
            db, name, &base_url, &username, &password, &member_id, &timezone,
        )
        .await?;
        tracing::info!(club = name, "seeded club from environment");
    }

    Ok(())
}
