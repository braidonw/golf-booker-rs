//! Club management pages: list, add, edit, delete.

use super::app::AppState;
use crate::error::AppError;
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use std::sync::Arc;

/// Timezone suggestions offered in the form (free text is still allowed and
/// validated server-side).
const TIMEZONES: &[&str] = &[
    "Australia/Sydney",
    "Australia/Melbourne",
    "Australia/Brisbane",
    "Australia/Perth",
    "Australia/Adelaide",
    "Pacific/Auckland",
    "Europe/London",
    "America/New_York",
    "UTC",
];

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/clubs", get(get::list).post(post::create))
        .route("/clubs/{id}/edit", get(get::edit))
        .route("/clubs/{id}", post(post::update))
        .route("/clubs/{id}/delete", post(post::delete))
        .with_state(state)
}

/// A club rendered to the list — credentials deliberately excluded.
struct ClubRow {
    id: i64,
    name: String,
    base_url: String,
    timezone: String,
}

impl From<&crate::clubs::Club> for ClubRow {
    fn from(c: &crate::clubs::Club) -> Self {
        Self {
            id: c.id,
            name: c.name.clone(),
            base_url: c.base_url.clone(),
            timezone: c.timezone.clone(),
        }
    }
}

/// Form field state, reused to prefill the form on validation error. Password is
/// never prefilled.
#[derive(Default)]
struct ClubFields {
    name: String,
    base_url: String,
    username: String,
    member_id: String,
    timezone: String,
}

#[derive(Template)]
#[template(path = "clubs/list.html")]
struct ListTemplate {
    username: String,
    clubs: Vec<ClubRow>,
    form: ClubFields,
    error: Option<String>,
    timezones: &'static [&'static str],
}

#[derive(Template)]
#[template(path = "clubs/edit.html")]
struct EditTemplate {
    username: String,
    id: i64,
    form: ClubFields,
    error: Option<String>,
    timezones: &'static [&'static str],
}

fn username_of(auth: &AuthSession) -> String {
    auth.user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "?".to_string())
}

/// Validate an IANA timezone string.
fn valid_timezone(tz: &str) -> bool {
    tz.parse::<chrono_tz::Tz>().is_ok()
}

mod get {
    use super::*;

    pub async fn list(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
    ) -> Result<Response, AppError> {
        let add_defaults = ClubFields {
            timezone: "Australia/Sydney".to_string(),
            ..ClubFields::default()
        };
        render_list(&state, username_of(&auth), add_defaults, None)
            .await
            .map(IntoResponse::into_response)
    }

    pub async fn edit(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path(id): Path<i64>,
    ) -> Result<Response, AppError> {
        let club = crate::clubs::get(&state.db, id)
            .await?
            .ok_or_else(|| AppError::not_found(anyhow::anyhow!("club {id} not found")))?;

        Ok(render(&EditTemplate {
            username: username_of(&auth),
            id,
            form: ClubFields {
                name: club.name.clone(),
                base_url: club.base_url.clone(),
                username: club.username.clone(),
                member_id: club.member_id.clone(),
                timezone: club.timezone.clone(),
            },
            error: None,
            timezones: TIMEZONES,
        })?
        .into_response())
    }
}

/// Shared helper to render the list page (also used to re-show it with an
/// error and a prefilled add-form).
async fn render_list(
    state: &AppState,
    username: String,
    form: ClubFields,
    error: Option<String>,
) -> Result<impl IntoResponse, AppError> {
    let clubs = crate::clubs::list(&state.db)
        .await?
        .iter()
        .map(ClubRow::from)
        .collect();

    render(&ListTemplate {
        username,
        clubs,
        form,
        error,
        timezones: TIMEZONES,
    })
}

mod post {
    use super::*;

    #[derive(Deserialize)]
    pub struct CreateForm {
        name: String,
        base_url: String,
        username: String,
        password: String,
        member_id: String,
        timezone: String,
    }

    pub async fn create(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Form(form): Form<CreateForm>,
    ) -> Result<Response, AppError> {
        if !valid_timezone(&form.timezone) {
            return render_list(
                &state,
                username_of(&auth),
                ClubFields {
                    name: form.name,
                    base_url: form.base_url,
                    username: form.username,
                    member_id: form.member_id,
                    timezone: form.timezone,
                },
                Some("Unknown timezone — use an IANA name like Australia/Sydney.".into()),
            )
            .await
            .map(IntoResponse::into_response);
        }

        crate::clubs::create(
            &state.db,
            &form.name,
            &form.base_url,
            &form.username,
            &form.password,
            &form.member_id,
            &form.timezone,
        )
        .await?;

        Ok(Redirect::to("/clubs").into_response())
    }

    #[derive(Deserialize)]
    pub struct UpdateForm {
        name: String,
        base_url: String,
        username: String,
        /// Blank means "keep existing password".
        password: String,
        member_id: String,
        timezone: String,
    }

    pub async fn update(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path(id): Path<i64>,
        Form(form): Form<UpdateForm>,
    ) -> Result<Response, AppError> {
        if !valid_timezone(&form.timezone) {
            return Ok(render(&EditTemplate {
                username: username_of(&auth),
                id,
                form: ClubFields {
                    name: form.name,
                    base_url: form.base_url,
                    username: form.username,
                    member_id: form.member_id,
                    timezone: form.timezone,
                },
                error: Some("Unknown timezone — use an IANA name like Australia/Sydney.".into()),
                timezones: TIMEZONES,
            })?
            .into_response());
        }

        let password = if form.password.is_empty() {
            None
        } else {
            Some(form.password.as_str())
        };

        crate::clubs::update(
            &state.db,
            id,
            &form.name,
            &form.base_url,
            &form.username,
            password,
            &form.member_id,
            &form.timezone,
        )
        .await?;

        Ok(Redirect::to("/clubs").into_response())
    }

    pub async fn delete(
        State(state): State<Arc<AppState>>,
        Path(id): Path<i64>,
        headers: HeaderMap,
    ) -> Result<Response, AppError> {
        crate::clubs::delete(&state.db, id).await?;

        // HTMX swaps the deleted row out; a plain form submit redirects back.
        if headers.contains_key("hx-request") {
            Ok(([("content-type", "text/html")], "").into_response())
        } else {
            Ok(Redirect::to("/clubs").into_response())
        }
    }
}
