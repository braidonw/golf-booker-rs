//! Authenticated pages (everything behind the login wall).

use super::app::AppState;
use crate::error::AppError;
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{response::Response, routing::get, Router};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new().route("/", get(get::index)).with_state(state)
}

mod get {
    use super::*;
    use axum::{extract::State, response::IntoResponse};

    #[derive(Template)]
    #[template(path = "home.html")]
    struct HomeTemplate {
        username: String,
        dry_run: bool,
    }

    pub async fn index(
        auth_session: AuthSession,
        State(state): State<Arc<AppState>>,
    ) -> Result<Response, AppError> {
        // `login_required` guarantees a user here, but handle the absence safely.
        let username = auth_session
            .user
            .map(|u| u.username)
            .unwrap_or_else(|| "?".to_string());

        Ok(render(&HomeTemplate {
            username,
            dry_run: state.config.dry_run,
        })?
        .into_response())
    }
}
