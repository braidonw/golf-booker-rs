//! User management: list, add, delete login accounts.

use super::app::AppState;
use crate::error::AppError;
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/users", get(get::list).post(post::create))
        .route("/users/{id}/delete", post(post::delete))
        .with_state(state)
}

struct UserView {
    id: i64,
    username: String,
    email: String,
    is_self: bool,
}

#[derive(Template)]
#[template(path = "users/list.html")]
struct ListTemplate {
    username: String,
    users: Vec<UserView>,
    error: Option<String>,
}

fn current(auth: &AuthSession) -> (i64, String) {
    auth.user
        .as_ref()
        .map(|u| (u.id, u.username.clone()))
        .unwrap_or((0, "?".to_string()))
}

async fn render_list(
    state: &AppState,
    auth: &AuthSession,
    error: Option<String>,
) -> Result<Response, AppError> {
    let (me, username) = current(auth);
    let users = crate::users::list(&state.db)
        .await?
        .into_iter()
        .map(|u| UserView {
            id: u.id,
            username: u.username,
            email: u.email.unwrap_or_default(),
            is_self: u.id == me,
        })
        .collect();

    Ok(render(&ListTemplate {
        username,
        users,
        error,
    })?
    .into_response())
}

mod get {
    use super::*;

    pub async fn list(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
    ) -> Result<Response, AppError> {
        render_list(&state, &auth, None).await
    }
}

mod post {
    use super::*;

    #[derive(Deserialize)]
    pub struct CreateForm {
        username: String,
        email: String,
        password: String,
    }

    pub async fn create(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Form(form): Form<CreateForm>,
    ) -> Result<Response, AppError> {
        let username = form.username.trim();
        let email = form.email.trim();
        if username.is_empty() {
            return render_list(&state, &auth, Some("Username is required.".into())).await;
        }
        if form.password.len() < crate::users::MIN_PASSWORD_LEN {
            return render_list(
                &state,
                &auth,
                Some(format!(
                    "Password must be at least {} characters.",
                    crate::users::MIN_PASSWORD_LEN
                )),
            )
            .await;
        }

        match crate::users::create(&state.db, username, Some(email), &form.password).await {
            Ok(_) => Ok(Redirect::to("/users").into_response()),
            // Most likely a duplicate username/email (UNIQUE constraint).
            Err(_) => {
                render_list(
                    &state,
                    &auth,
                    Some("That username or email is already taken.".into()),
                )
                .await
            }
        }
    }

    pub async fn delete(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path(id): Path<i64>,
    ) -> Result<Response, AppError> {
        let (me, _) = current(&auth);
        if id == me {
            return render_list(
                &state,
                &auth,
                Some("You can't delete your own account.".into()),
            )
            .await;
        }
        if crate::users::count(&state.db).await? <= 1 {
            return render_list(&state, &auth, Some("Can't delete the last account.".into())).await;
        }
        crate::users::delete(&state.db, id).await?;
        Ok(Redirect::to("/users").into_response())
    }
}
