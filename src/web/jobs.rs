//! Scheduled-booking pages: list and cancel. Creation happens from a slot's
//! "Schedule" button (see `web::events`), which posts here.

use super::app::AppState;
use crate::error::AppError;
use crate::scheduler::JobData;
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

/// Interpret a `datetime-local` value as wall-clock time **in the club's
/// timezone** and convert to UTC. `None` for an unparseable or non-existent
/// local time (spring-forward gap).
///
/// The reference is deliberately the *club's* IANA zone, not the user's browser
/// timezone: a tee sheet opens at the club's local time, and the operator may
/// schedule a club in a different zone than they're sitting in (e.g. a Sydney
/// club from London). Sending the browser's time/offset up would book at the
/// wrong instant whenever the two zones differ, so the club zone is authoritative.
fn local_to_utc(value: &str, tz: Tz) -> Option<DateTime<Utc>> {
    let naive = parse_local_datetime(value)?;
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        // DST fall-back: this wall-clock time occurs twice. Fire at the earlier
        // of the two instants — for a booking race, being early is harmless
        // (we pre-auth and wait), being late forfeits it. Chosen explicitly
        // rather than relying on the tuple order.
        chrono::LocalResult::Ambiguous(a, b) => {
            let earlier = a.min(b).with_timezone(&Utc);
            tracing::warn!(
                local = %naive, tz = %tz, chosen = %earlier,
                "ambiguous local time (DST fall-back) — using the earlier instant"
            );
            Some(earlier)
        }
        // DST spring-forward gap: this wall-clock time never occurs, so there is
        // no instant to schedule. The caller surfaces this as a "bad time" error.
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

#[derive(Template)]
#[template(path = "jobs/list.html")]
struct ListTemplate {
    username: String,
    jobs: Vec<JobRow>,
}

mod get {
    use super::*;

    pub async fn list(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
    ) -> Result<Response, AppError> {
        let uid = user_id(&auth).ok_or_else(|| anyhow::anyhow!("not authenticated"))?;

        // Club name + timezone lookup for rendering local times.
        let club_meta: HashMap<i64, (String, Tz)> = crate::clubs::list(&state.db)
            .await?
            .into_iter()
            .map(|c| (c.id, (c.name, c.timezone.parse().unwrap_or(Tz::UTC))))
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

        Ok(render(&ListTemplate {
            username: username_of(&auth),
            jobs,
        })?
        .into_response())
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

        // On any validation problem, bounce back to the slot's schedule page
        // with an error code (it has the context to re-render).
        let back = |code: &str| {
            Redirect::to(&format!(
                "/clubs/{}/events/{}/groups/{}/schedule?error={code}",
                form.club_id, form.event_id, form.booking_group_id
            ))
            .into_response()
        };

        let Some(club) = crate::clubs::get(&state.db, form.club_id).await? else {
            return Ok(back("unknownclub"));
        };
        let tz: Tz = club.timezone.parse().unwrap_or(Tz::UTC);

        let Some(when_utc) = local_to_utc(&form.scheduled_time, tz) else {
            return Ok(back("badtime"));
        };
        if when_utc <= Utc::now() {
            return Ok(back("past"));
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
    fn ambiguous_fall_back_time_picks_the_earlier_instant() {
        // Australia/Sydney DST ends 05 Apr 2026: at 03:00 AEDT clocks fall back
        // to 02:00 AEST, so 02:30 occurs twice. The earlier instant is the AEDT
        // (UTC+11) one: 02:30 +11 == 15:30 UTC the previous day.
        let utc = local_to_utc("2026-04-05T02:30", Tz::Australia__Sydney).unwrap();
        assert_eq!(utc.to_rfc3339(), "2026-04-04T15:30:00+00:00");
    }

    #[test]
    fn spring_forward_gap_time_is_rejected() {
        // Australia/Sydney DST starts 04 Oct 2026: at 02:00 AEST clocks jump to
        // 03:00 AEDT, so 02:30 never exists that day.
        assert!(local_to_utc("2026-10-04T02:30", Tz::Australia__Sydney).is_none());
    }

    #[test]
    fn display_roundtrips_into_zone() {
        let shown = utc_to_local_display("2026-06-20T00:00:00+00:00", Tz::Australia__Sydney);
        assert!(shown.contains("10:00"), "got: {shown}");
    }
}
