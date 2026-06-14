//! Login / logout and the password-reset flow (all public, outside the gate).

use super::app::AppState;
use crate::error::AppError;
use crate::users::{AuthSession, Credentials};
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    message: Option<String>,
    next: Option<String>,
}

#[derive(Template)]
#[template(path = "forgot.html")]
struct ForgotTemplate {
    message: Option<String>,
}

#[derive(Template)]
#[template(path = "reset.html")]
struct ResetTemplate {
    token: String,
    message: Option<String>,
    done: bool,
}

#[derive(Deserialize)]
pub struct NextUrl {
    next: Option<String>,
}

/// Only allow same-origin, relative redirect targets (no open redirects).
fn safe_next(next: Option<&str>) -> Option<String> {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\") => {
            Some(n.to_string())
        }
        _ => None,
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/login", get(get::login).post(post::login))
        .route("/logout", post(post::logout))
        .route("/forgot", get(get::forgot).post(post::forgot))
        .route("/reset", get(get::reset).post(post::reset))
        .with_state(state)
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

    pub async fn forgot() -> Result<Response, AppError> {
        Ok(render(&ForgotTemplate { message: None })?.into_response())
    }

    #[derive(Deserialize)]
    pub struct ResetQuery {
        token: Option<String>,
    }

    pub async fn reset(Query(q): Query<ResetQuery>) -> Result<Response, AppError> {
        Ok(render(&ResetTemplate {
            token: q.token.unwrap_or_default(),
            message: None,
            done: false,
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

    #[derive(Deserialize)]
    pub struct ForgotForm {
        email: String,
    }

    pub async fn forgot(
        State(state): State<Arc<AppState>>,
        Form(form): Form<ForgotForm>,
    ) -> Result<Response, AppError> {
        let email = form.email.trim();

        // If the email maps to an account, create a token and send the link.
        // Either way we respond identically, to avoid revealing who has an account.
        if let Some(user_id) = crate::users::id_for_email(&state.db, email).await? {
            let token = crate::users::create_reset_token(&state.db, user_id).await?;
            let link = format!("{}/reset?token={token}", state.config.base_url);
            let body = format!(
                "Someone requested a password reset for your golf-booker account.\n\n\
                 Reset it here (expires in 1 hour):\n{link}\n\n\
                 If this wasn't you, you can ignore this email.\n"
            );
            match state
                .mailer
                .send(email, "Reset your golf-booker password", body)
                .await
            {
                Ok(true) => tracing::info!("sent password-reset email"),
                // Dev fallback: no SMTP configured, so surface the link in logs.
                Ok(false) => tracing::warn!("SMTP disabled — reset link: {link}"),
                Err(e) => tracing::error!("failed to send reset email: {e}"),
            }
        }

        Ok(render(&ForgotTemplate {
            message: Some("If that email is registered, a reset link is on its way.".to_string()),
        })?
        .into_response())
    }

    #[derive(Deserialize)]
    pub struct ResetForm {
        token: String,
        password: String,
    }

    pub async fn reset(
        State(state): State<Arc<AppState>>,
        Form(form): Form<ResetForm>,
    ) -> Result<Response, AppError> {
        if form.password.len() < crate::users::MIN_PASSWORD_LEN {
            return Ok(render(&ResetTemplate {
                token: form.token,
                message: Some(format!(
                    "Password must be at least {} characters.",
                    crate::users::MIN_PASSWORD_LEN
                )),
                done: false,
            })?
            .into_response());
        }

        match crate::users::consume_reset_token(&state.db, &form.token).await? {
            Some(user_id) => {
                crate::users::set_password(&state.db, user_id, &form.password).await?;
                Ok(render(&ResetTemplate {
                    token: String::new(),
                    message: Some("Password updated — you can sign in now.".to_string()),
                    done: true,
                })?
                .into_response())
            }
            None => Ok(render(&ResetTemplate {
                token: form.token,
                message: Some("That reset link is invalid or has expired.".to_string()),
                done: false,
            })?
            .into_response()),
        }
    }
}
