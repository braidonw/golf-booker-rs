//! Login / logout HTTP handlers.

use crate::error::AppError;
use crate::users::{AuthSession, Credentials};
use crate::web::render;
use askama::Template;
use axum::{
    extract::Query,
    response::{IntoResponse, Redirect, Response},
    routing::get,
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

pub fn router() -> Router<()> {
    Router::new()
        .route("/login", get(get::login).post(post::login))
        .route("/logout", get(get::logout))
}

mod get {
    use super::*;

    pub async fn login(Query(NextUrl { next }): Query<NextUrl>) -> Result<Response, AppError> {
        Ok(render(&LoginTemplate {
            message: None,
            next,
        })?
        .into_response())
    }

    pub async fn logout(mut auth_session: AuthSession) -> Result<Response, AppError> {
        auth_session.logout().await?;
        Ok(Redirect::to("/login").into_response())
    }
}

mod post {
    use super::*;

    pub async fn login(
        mut auth_session: AuthSession,
        Form(creds): Form<Credentials>,
    ) -> Result<Response, AppError> {
        let user = match auth_session.authenticate(creds.clone()).await? {
            Some(user) => user,
            None => {
                return Ok(render(&LoginTemplate {
                    message: Some("Invalid username or password.".to_string()),
                    next: creds.next,
                })?
                .into_response());
            }
        };

        auth_session.login(&user).await?;

        let dest = creds.next.as_deref().unwrap_or("/");
        Ok(Redirect::to(dest).into_response())
    }
}
