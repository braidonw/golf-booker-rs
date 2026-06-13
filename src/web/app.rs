//! HTTP application wiring: state, router, server.

use crate::config::Config;
use crate::error::AppError;
use crate::web::render;
use askama::Template;
use axum::{extract::State, response::IntoResponse, routing::get, Router};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

/// Shared application state handed to every request handler.
pub struct AppState {
    pub db: SqlitePool,
    pub config: Config,
}

/// Owns startup-time resources before the server is launched.
pub struct App {
    state: Arc<AppState>,
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    dry_run: bool,
}

async fn home(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    render(&HomeTemplate {
        dry_run: state.config.dry_run,
    })
}

async fn health() -> &'static str {
    "ok"
}

impl App {
    pub async fn new() -> anyhow::Result<Self> {
        let config = Config::from_env();
        tracing::info!(dry_run = config.dry_run, "starting golf-booker");

        let db = crate::db::connect(&config.database_url).await?;

        Ok(Self {
            state: Arc::new(AppState { db, config }),
        })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let port = self.state.config.port;

        let app = Router::new()
            .route("/", get(home))
            .route("/health", get(health))
            .nest_service("/assets", ServeDir::new("assets"))
            .layer(TraceLayer::new_for_http())
            .with_state(self.state.clone());

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!(%addr, "listening");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
