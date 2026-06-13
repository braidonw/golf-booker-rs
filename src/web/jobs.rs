//! Scheduled-booking pages: list, create (timezone-aware), cancel.

use super::app::AppState;
use crate::error::AppError;
use crate::scheduler::JobData;
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/scheduled-jobs", get(get::list).post(post::create))
        .route("/scheduled-jobs/{id}/cancel", post(post::cancel))
        .with_state(state)
}

fn user_id(auth: &AuthSession) -> Option<i64> {
    auth.user.as_ref().map(|u| u.id)
}

fn username_of(auth: &AuthSession) -> String {
    auth.user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "?".to_string())
}

/// Parse a browser `datetime-local` value (with or without seconds).
fn parse_local_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))
        .ok()
}

/// Interpret a `datetime-local` value as wall-clock time in `tz` and convert to
/// UTC. Returns `None` for an unparseable or non-existent local time (e.g. a
/// spring-forward gap).
fn local_to_utc(value: &str, tz: Tz) -> Option<DateTime<Utc>> {
    let naive = parse_local_datetime(value)?;
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        // DST fall-back: two valid instants — take the earliest.
        chrono::LocalResult::Ambiguous(dt, _) => Some(dt.with_timezone(&Utc)),
        chrono::LocalResult::None => None,
    }
}

/// Format a stored RFC3339 UTC time in a club's local zone for display.
fn utc_to_local_display(rfc3339: &str, tz: Tz) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| {
            dt.with_timezone(&tz)
                .format("%a %d %b %Y, %H:%M %Z")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// A scheduled job rendered to the list.
struct JobRow {
    id: i64,
    club_name: String,
    event_id: i64,
    group_id: u32,
    scheduled_local: String,
    status: String,
    last_error: Option<String>,
    can_cancel: bool,
}

/// A club option in the create-form dropdown.
struct ClubOption {
    id: i64,
    name: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "jobs/list.html")]
struct ListTemplate {
    username: String,
    jobs: Vec<JobRow>,
    clubs: Vec<ClubOption>,
    error: Option<String>,
    // Prefill (e.g. arriving from a slot's "Schedule" button).
    prefill_event_id: Option<i64>,
    prefill_group_id: Option<u32>,
}

/// Prefill query params carried from a booking slot.
#[derive(Deserialize, Clone)]
pub struct Prefill {
    club_id: Option<i64>,
    event_id: Option<i64>,
    group_id: Option<u32>,
}

async fn render_list(
    state: &AppState,
    auth: &AuthSession,
    prefill: Prefill,
    error: Option<String>,
) -> Result<Response, AppError> {
    let uid = user_id(auth).ok_or_else(|| anyhow::anyhow!("not authenticated"))?;

    // Club name + timezone lookup for rendering local times and the dropdown.
    let clubs = crate::clubs::list(&state.db).await?;
    let club_meta: HashMap<i64, (String, Tz)> = clubs
        .iter()
        .map(|c| {
            (
                c.id,
                (c.name.clone(), c.timezone.parse().unwrap_or(Tz::UTC)),
            )
        })
        .collect();

    let jobs = state
        .scheduler
        .get_user_jobs(uid)
        .await?
        .into_iter()
        .map(|job| {
            let (club_name, tz) = job
                .club_id
                .and_then(|id| club_meta.get(&id).cloned())
                .unwrap_or_else(|| ("(removed)".to_string(), Tz::UTC));
            let group_id = match job.job_data() {
                Ok(JobData::Booking(d)) => d.booking_group_id,
                Err(_) => 0,
            };
            JobRow {
                id: job.id,
                club_name,
                event_id: job.event_id.unwrap_or_default(),
                group_id,
                scheduled_local: utc_to_local_display(&job.scheduled_time, tz),
                can_cancel: job.status == "pending",
                status: job.status,
                last_error: job.last_error,
            }
        })
        .collect();

    let club_options = clubs
        .iter()
        .map(|c| ClubOption {
            id: c.id,
            name: c.name.clone(),
            selected: prefill.club_id == Some(c.id),
        })
        .collect();

    Ok(render(&ListTemplate {
        username: username_of(auth),
        jobs,
        clubs: club_options,
        error,
        prefill_event_id: prefill.event_id,
        prefill_group_id: prefill.group_id,
    })?
    .into_response())
}

mod get {
    use super::*;

    pub async fn list(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Query(prefill): Query<Prefill>,
    ) -> Result<Response, AppError> {
        render_list(&state, &auth, prefill, None).await
    }
}

mod post {
    use super::*;

    #[derive(Deserialize)]
    pub struct CreateForm {
        club_id: i64,
        event_id: i64,
        booking_group_id: u32,
        /// `datetime-local`, interpreted in the chosen club's timezone.
        scheduled_time: String,
    }

    pub async fn create(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Form(form): Form<CreateForm>,
    ) -> Result<Response, AppError> {
        let Some(uid) = user_id(&auth) else {
            return Err(anyhow::anyhow!("not authenticated").into());
        };

        let prefill = Prefill {
            club_id: Some(form.club_id),
            event_id: Some(form.event_id),
            group_id: Some(form.booking_group_id),
        };

        let club = match crate::clubs::get(&state.db, form.club_id).await? {
            Some(c) => c,
            None => return render_list(&state, &auth, prefill, Some("Unknown club.".into())).await,
        };
        let tz: Tz = club.timezone.parse().unwrap_or(Tz::UTC);

        let Some(when_utc) = local_to_utc(&form.scheduled_time, tz) else {
            return render_list(
                &state,
                &auth,
                prefill,
                Some("Couldn't read that date/time.".into()),
            )
            .await;
        };
        if when_utc <= Utc::now() {
            return render_list(
                &state,
                &auth,
                prefill,
                Some("That time is in the past.".into()),
            )
            .await;
        }

        state
            .scheduler
            .schedule_booking(
                uid,
                form.club_id,
                form.event_id,
                form.booking_group_id,
                when_utc,
            )
            .await?;

        Ok(Redirect::to("/scheduled-jobs").into_response())
    }

    pub async fn cancel(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path(id): Path<i64>,
        headers: HeaderMap,
    ) -> Result<Response, AppError> {
        let Some(uid) = user_id(&auth) else {
            return Err(anyhow::anyhow!("not authenticated").into());
        };
        state.scheduler.cancel_job(uid, id).await?;

        if headers.contains_key("hx-request") {
            Ok(([("content-type", "text/html")], "").into_response())
        } else {
            Ok(Redirect::to("/scheduled-jobs").into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_to_utc_uses_club_timezone() {
        // 10:00 in Sydney (UTC+10 in June, no DST) is 00:00 UTC.
        let utc = local_to_utc("2026-06-20T10:00", Tz::Australia__Sydney).unwrap();
        assert_eq!(utc.to_rfc3339(), "2026-06-20T00:00:00+00:00");
    }

    #[test]
    fn local_to_utc_differs_by_zone() {
        let syd = local_to_utc("2026-06-20T10:00", Tz::Australia__Sydney).unwrap();
        let utc = local_to_utc("2026-06-20T10:00", Tz::UTC).unwrap();
        assert_ne!(syd, utc);
        assert_eq!((utc - syd).num_hours(), 10);
    }

    #[test]
    fn rejects_unparseable_time() {
        assert!(local_to_utc("not-a-time", Tz::UTC).is_none());
    }

    #[test]
    fn display_roundtrips_into_zone() {
        let shown = utc_to_local_display("2026-06-20T00:00:00+00:00", Tz::Australia__Sydney);
        assert!(shown.contains("10:00"), "got: {shown}");
    }
}
