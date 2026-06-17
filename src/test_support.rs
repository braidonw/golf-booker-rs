//! Shared helpers for the crate's database-backed tests.
//!
//! Each call to [`test_pool`] yields an isolated in-memory SQLite database with
//! all migrations applied, so DB-touching logic can be exercised without a real
//! file or a running server.

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// An isolated in-memory SQLite pool with migrations applied.
///
/// `max_connections(1)` keeps every query on the one in-memory database (each
/// extra `:memory:` connection would otherwise get its own empty database), and
/// foreign-key enforcement is on by default in sqlx, so the schema's `REFERENCES`
/// constraints are exercised too.
pub async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

/// Insert a login account directly and return its id (bypasses argon2 hashing
/// cost when a test just needs a valid `user_id` for a foreign key).
pub async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query("INSERT INTO users (username, password) VALUES (?, 'x')")
        .bind(username)
        .execute(pool)
        .await
        .expect("seed user")
        .last_insert_rowid()
}

/// Insert a club directly and return its id, for tests that just need a valid
/// `club_id`.
pub async fn seed_club(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query(
        "INSERT INTO clubs (name, base_url, username, password, member_id, timezone) \
         VALUES (?, 'https://example.com', 'u', 'p', 'm', 'Australia/Sydney')",
    )
    .bind(name)
    .execute(pool)
    .await
    .expect("seed club")
    .last_insert_rowid()
}
