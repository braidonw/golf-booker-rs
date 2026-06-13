//! Login / logout HTTP handlers.

use crate::error::AppError;
use crate::users::{AuthSession, Credentials};
use crate::web::render;
use askama::Template;
use axum::{
    extract::Query,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    message: Option<String>,
    next: Option<String>,
}

/// `?next=/somewhere` carried through the login form so we can return the user
/// to where they were headed.
#[derive(Deserialize)]
pub struct NextUrl {
    next: Option<String>,
}

/// Only allow same-origin, relative redirect targets, to prevent open-redirect
/// abuse via a crafted `next` (e.g. `//evil.com` or `https://evil.com`).
fn safe_next(next: Option<&str>) -> Option<String> {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\") => {
            Some(n.to_string())
        }
        _ => None,
    }
}

pub fn router() -> Router<()> {
    Router::new()
        .route("/login", get(get::login).post(post::login))
        .route("/logout", post(post::logout))
}

mod get {
    use super::*;

    pub async fn login(Query(NextUrl { next }): Query<NextUrl>) -> Result<Response, AppError> {
        Ok(render(&LoginTemplate {
            message: None,
            next: safe_next(next.as_deref()),
        })?
        .into_response())
    }
}

mod post {
    use super::*;

    pub async fn login(
        mut auth_session: AuthSession,
        Form(creds): Form<Credentials>,
    ) -> Result<Response, AppError> {
        let next = safe_next(creds.next.as_deref());
        let limiter = crate::web::ratelimit::login_limiter();
        let key = creds.username.to_lowercase();

        // Throttle repeated failures for a username before doing any work.
        if !limiter.allowed(&key) {
            return Ok(render(&LoginTemplate {
                message: Some("Too many attempts. Wait a few minutes and try again.".to_string()),
                next,
            })?
            .into_response());
        }

        let user = match auth_session.authenticate(creds.clone()).await? {
            Some(user) => user,
            None => {
                limiter.record_failure(&key);
                return Ok(render(&LoginTemplate {
                    message: Some("Invalid username or password.".to_string()),
                    next,
                })?
                .into_response());
            }
        };

        auth_session.login(&user).await?;
        limiter.record_success(&key);

        Ok(Redirect::to(next.as_deref().unwrap_or("/")).into_response())
    }

    pub async fn logout(mut auth_session: AuthSession) -> Result<Response, AppError> {
        auth_session.logout().await?;
        Ok(Redirect::to("/login").into_response())
    }
}
