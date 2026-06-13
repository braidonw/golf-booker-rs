//! HTTP application wiring: state, router, server.

use crate::config::Config;
use crate::users::Backend;
use axum::{routing::get, Router};
use axum_login::{
    login_required,
    tower_sessions::{Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::{time::Duration, SameSite};
use tower_sessions_sqlx_store::SqliteStore;

use super::{auth, clubs, protected};

/// Shared application state handed to every request handler.
pub struct AppState {
    pub db: SqlitePool,
    pub config: Config,
}

/// Owns startup-time resources before the server is launched.
pub struct App {
    state: Arc<AppState>,
}

async fn health() -> &'static str {
    "ok"
}

impl App {
    pub async fn new() -> anyhow::Result<Self> {
        let config = Config::from_env();
        tracing::info!(dry_run = config.dry_run, "starting golf-booker");

        let db = crate::db::connect(&config.database_url).await?;

        // Seed the first login account and any clubs from the environment if
        // none exist yet (transparent migration from an env-only setup).
        crate::users::seed_from_environment(&db).await?;
        crate::clubs::seed_from_environment(&db).await?;

        Ok(Self {
            state: Arc::new(AppState { db, config }),
        })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let port = self.state.config.port;
        let db = self.state.db.clone();

        // Persistent session store (survives restarts), sharing our pool.
        let session_store = SqliteStore::new(db.clone());
        session_store.migrate().await?;
        let session_layer = SessionManagerLayer::new(session_store)
            .with_secure(self.state.config.cookie_secure)
            .with_http_only(true)
            .with_same_site(SameSite::Lax)
            .with_expiry(Expiry::OnInactivity(Duration::days(7)));

        // Auth layer: combines sessions with our credential backend. The layer
        // is infallible, so no HandleError wrapper is needed.
        let backend = Backend::new(db.clone());
        let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

        let app = Router::new()
            .merge(protected::router(self.state.clone()))
            .merge(clubs::router(self.state.clone()))
            .route_layer(login_required!(Backend, login_url = "/login"))
            .merge(auth::router())
            .route("/health", get(health))
            .nest_service("/assets", ServeDir::new("assets"))
            .layer(auth_layer)
            .layer(TraceLayer::new_for_http());

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!(%addr, "listening");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
