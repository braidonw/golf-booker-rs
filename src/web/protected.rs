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

    struct ClubLink {
        id: i64,
        name: String,
    }

    #[derive(Template)]
    #[template(path = "home.html")]
    struct HomeTemplate {
        username: String,
        dry_run: bool,
        clubs: Vec<ClubLink>,
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

        let clubs = crate::clubs::list(&state.db)
            .await?
            .into_iter()
            .map(|c| ClubLink {
                id: c.id,
                name: c.name,
            })
            .collect();

        Ok(render(&HomeTemplate {
            username,
            dry_run: state.config.dry_run,
            clubs,
        })?
        .into_response())
    }
}
