//! HTTP application wiring: state, router, server.

use crate::clubs::Club;
use crate::config::Config;
use crate::email::Mailer;
use crate::golf::GolfClient;
use crate::scheduler::JobScheduler;
use crate::users::Backend;
use axum::{routing::get, Router};
use axum_login::{
    login_required,
    tower_sessions::{Expiry, SessionManagerLayer},
    AuthManagerLayerBuilder,
};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::{time::Duration, SameSite};
use tower_sessions_sqlx_store::SqliteStore;

use super::{auth, clubs, events, jobs, protected, users};

/// Shared application state handed to every request handler.
pub struct AppState {
    pub db: SqlitePool,
    pub config: Config,
    pub scheduler: JobScheduler,
    pub mailer: Mailer,
    /// One authenticated [`GolfClient`] per club id, reused across requests so
    /// browsing doesn't re-login to the club on every page view. A client owns
    /// its cookie jar; cached clones share it, so a re-login on any clone (see
    /// `web::events`) refreshes the session for all of them.
    club_clients: Mutex<HashMap<i64, GolfClient>>,
}

impl AppState {
    /// A logged-in client for `club`, reusing the cached one if present.
    /// Logs in only when creating a fresh entry; the web layer re-authenticates
    /// on a failed request to recover a lapsed session.
    pub async fn club_client(&self, club: &Club) -> anyhow::Result<GolfClient> {
        // Fast path: a cached hit, without holding the lock across any I/O.
        if let Some(client) = self.club_clients.lock().await.get(&club.id) {
            return Ok(client.clone());
        }
        // Build and log in *outside* the lock, so a slow login to one club
        // doesn't block lookups for the others.
        let client = GolfClient::from_club(club);
        client.login().await?;
        // If another request raced us and inserted first, keep that one so all
        // callers share a single cookie jar (our extra login is just discarded).
        let mut cache = self.club_clients.lock().await;
        Ok(cache.entry(club.id).or_insert(client).clone())
    }

    /// Drop a club's cached client, so the next request rebuilds and re-logs in.
    /// Called when a club's credentials change or it's removed.
    pub async fn invalidate_club_client(&self, club_id: i64) {
        self.club_clients.lock().await.remove(&club_id);
    }

    /// Build an `AppState` for tests with an empty client cache (the cache field
    /// is private, so tests can't use a struct literal).
    #[cfg(test)]
    pub(crate) fn for_test(
        db: SqlitePool,
        config: Config,
        scheduler: JobScheduler,
        mailer: Mailer,
    ) -> Self {
        Self {
            db,
            config,
            scheduler,
            mailer,
            club_clients: Mutex::new(HashMap::new()),
        }
    }
}

/// Owns startup-time resources before the server is launched.
pub struct App {
    state: Arc<AppState>,
}

async fn health() -> &'static str {
    "ok"
}

/// Assemble the full application router with session + auth layering. Shared by
/// [`App::serve`] and the integration tests, so the tests exercise the real
/// routing, the login gate, and the public/private split — not a stand-in.
pub(crate) async fn build_router(state: Arc<AppState>) -> anyhow::Result<Router> {
    // Persistent session store (survives restarts), sharing our pool.
    let session_store = SqliteStore::new(state.db.clone());
    session_store.migrate().await?;
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(state.config.cookie_secure)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(Duration::days(7)));

    // Auth layer: combines sessions with our credential backend. The layer is
    // infallible, so no HandleError wrapper is needed.
    let backend = Backend::new(state.db.clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();

    Ok(Router::new()
        .merge(protected::router(state.clone()))
        .merge(clubs::router(state.clone()))
        .merge(events::router(state.clone()))
        .merge(jobs::router(state.clone()))
        .merge(users::router(state.clone()))
        .route_layer(login_required!(Backend, login_url = "/login"))
        .merge(auth::router(state.clone()))
        .route("/health", get(health))
        .nest_service("/assets", ServeDir::new("assets"))
        .layer(auth_layer)
        .layer(TraceLayer::new_for_http()))
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

        let mailer = Mailer::from_config(config.smtp.as_ref())?;
        let scheduler = JobScheduler::new(
            db.clone(),
            config.dry_run,
            mailer.clone(),
            config.base_url.clone(),
        );

        Ok(Self {
            state: Arc::new(AppState {
                db,
                config,
                scheduler,
                mailer,
                club_clients: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let port = self.state.config.port;

        // Launch the background scheduler dispatcher.
        self.state.scheduler.start().await;

        let app = build_router(self.state.clone()).await?;

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!(%addr, "listening");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
