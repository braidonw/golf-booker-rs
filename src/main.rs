use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod clubs;
mod config;
mod db;
mod error;
mod users;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env in dev; ignored if absent (prod sets real env vars).
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(
            |_| "golf_booker=info,tower_http=info,axum_login=info,sqlx=warn".into(),
        )))
        .with(tracing_subscriber::fmt::layer())
        .try_init()?;

    web::App::new().await?.serve().await
}
