//! Multi-club event browsing, immediate ("book now") booking, and the
//! "schedule this booking" page (the only entry point to scheduling).

use super::app::AppState;
use crate::clubs::Club;
use crate::error::AppError;
use crate::golf::{BookingEvent, GolfClient, GolfEvent};
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/clubs/{club_id}/events", get(get::list))
        .route("/clubs/{club_id}/events/{event_id}", get(get::detail))
        .route(
            "/clubs/{club_id}/events/{event_id}/groups/{group_id}/book",
            post(post::book),
        )
        .route(
            "/clubs/{club_id}/events/{event_id}/groups/{group_id}/schedule",
            get(get::schedule),
        )
        .with_state(state)
}

/// Minimal club info for the switcher shown on browsing pages.
struct ClubLink {
    id: i64,
    name: String,
}

fn username_of(auth: &AuthSession) -> String {
    auth.user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "?".to_string())
}

async fn club_links(state: &AppState) -> Result<Vec<ClubLink>, AppError> {
    Ok(crate::clubs::list(&state.db)
        .await?
        .into_iter()
        .map(|c| ClubLink {
            id: c.id,
            name: c.name,
        })
        .collect())
}

async fn load_club(state: &AppState, club_id: i64) -> Result<Club, AppError> {
    crate::clubs::get(&state.db, club_id)
        .await?
        .ok_or_else(|| AppError::not_found(anyhow::anyhow!("club {club_id} not found")))
}

/// Run a read against the club, re-authenticating once if it fails. The cached
/// client (see `AppState::club_client`) may carry a session that has since
/// lapsed; a re-login refreshes its shared cookie jar, so the retry — and later
/// cache hits — succeed. Used only for idempotent reads, never for `book`.
async fn with_relogin<F, Fut, T>(client: &GolfClient, op: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    match op().await {
        Ok(value) => Ok(value),
        Err(first) => {
            if let Err(e) = client.login().await {
                return Err(
                    first.context(format!("re-login after a failed request also failed: {e}"))
                );
            }
            op().await
        }
    }
}

/// A success/error banner shown after a booking attempt.
struct Flash {
    ok: bool,
    text: String,
}

/// A compact summary of an event's metadata, shown on the detail page.
struct EventSummary {
    status: String,
    gender: String,
    availability: u32,
    type_code: Option<u32>,
    category_code: Option<u32>,
    time_code: Option<String>,
    flags: Vec<&'static str>,
    is_open: bool,
}

impl From<&GolfEvent> for EventSummary {
    fn from(e: &GolfEvent) -> Self {
        let mut flags = Vec::new();
        if e.is_lottery == Some(true) {
            flags.push("Lottery");
        }
        if e.has_competition == Some(true) {
            flags.push("Competition");
        }
        if e.is_matchplay {
            flags.push("Matchplay");
        }
        if e.is_ballot_open {
            flags.push("Ballot open");
        }
        if e.is_results {
            flags.push("Results posted");
        }
        Self {
            status: e.status(),
            gender: e.gender_label().to_string(),
            availability: e.availability,
            type_code: e.event_type_code,
            category_code: e.event_category_code,
            time_code: e.event_time_code_friendly.clone(),
            flags,
            is_open: e.is_open,
        }
    }
}

/// Event detail with its booking sheet. Rendered by the GET view and the POST
/// book handler (the latter adds a `flash`), so it lives at module scope.
#[derive(Template)]
#[template(path = "events/detail.html")]
struct DetailTemplate {
    username: String,
    club_id: i64,
    club_name: String,
    event: BookingEvent,
    summary: Option<EventSummary>,
    /// Whether the tee sheet is open for booking now (from the event
    /// metadata). Decides Book-now vs Schedule per slot. Unknown metadata is
    /// treated as not-open, so we don't offer an immediate booking we can't
    /// vouch for.
    event_open: bool,
    flash: Option<Flash>,
}

/// Fetch the sheet + metadata for an event and render the detail page.
async fn render_detail(
    username: String,
    club_id: i64,
    club_name: String,
    client: &GolfClient,
    event_id: u32,
    flash: Option<Flash>,
) -> Result<Response, AppError> {
    let event = with_relogin(client, || client.get_event(event_id)).await?;
    // Metadata is best-effort — a detail page is still useful without it. Query
    // just the event's own day (we already have its date) rather than the full
    // listing window.
    let meta_date = event.date.date();
    let summary = with_relogin(client, || client.get_event_meta_on(event_id, meta_date))
        .await
        .ok()
        .flatten()
        .map(|m| EventSummary::from(&m));
    let event_open = summary.as_ref().is_some_and(|s| s.is_open);

    Ok(render(&DetailTemplate {
        username,
        club_id,
        club_name,
        event,
        summary,
        event_open,
        flash,
    })?
    .into_response())
}

mod get {
    use super::*;

    #[derive(Template)]
    #[template(path = "events/list.html")]
    struct ListTemplate {
        username: String,
        club_id: i64,
        club_name: String,
        clubs: Vec<ClubLink>,
        events: Vec<GolfEvent>,
        error: Option<String>,
    }

    pub async fn list(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path(club_id): Path<i64>,
    ) -> Result<Response, AppError> {
        let club = load_club(&state, club_id).await?;
        let clubs = club_links(&state).await?;

        // A failure talking to the club shouldn't 500 the page — show it inline.
        let (events, error) = match fetch_events(&state, &club).await {
            Ok(events) => (events, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };

        Ok(render(&ListTemplate {
            username: username_of(&auth),
            club_id,
            club_name: club.name.clone(),
            clubs,
            events,
            error,
        })?
        .into_response())
    }

    pub async fn detail(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path((club_id, event_id)): Path<(i64, u32)>,
    ) -> Result<Response, AppError> {
        let club = load_club(&state, club_id).await?;
        let client = state.club_client(&club).await?;
        render_detail(
            username_of(&auth),
            club_id,
            club.name,
            &client,
            event_id,
            None,
        )
        .await
    }

    #[derive(Template)]
    #[template(path = "events/schedule.html")]
    struct ScheduleTemplate {
        username: String,
        club_id: i64,
        club_name: String,
        club_tz: String,
        event_id: u32,
        group_id: u32,
        event_title: String,
        event_date: String,
        slot_time: Option<String>,
        slot_holes: Option<u32>,
        slot_booked: Option<usize>,
        slot_size: Option<u32>,
        slot_full: bool,
        default_time: String,
        opens_hint: Option<String>,
        error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    pub struct ScheduleQuery {
        error: Option<String>,
    }

    pub async fn schedule(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path((club_id, event_id, group_id)): Path<(i64, u32, u32)>,
        Query(q): Query<ScheduleQuery>,
    ) -> Result<Response, AppError> {
        let club = load_club(&state, club_id).await?;
        let client = state.club_client(&club).await?;

        let event = with_relogin(&client, || client.get_event(event_id)).await?;
        let group = event.find_group(group_id);
        // A full slot can't be scheduled — there's no seat to race for. (The
        // detail page hides the button, so this only bites a direct URL.)
        let slot_full = group.is_some_and(|g| !g.is_schedulable());
        let (slot_time, slot_holes, slot_booked, slot_size) = match group {
            Some(g) => (
                Some(g.time.clone()),
                g.holes(),
                Some(g.entry_count()),
                Some(g.size),
            ),
            None => (None, None, None, None),
        };

        // Metadata gives us the auto-open time to pre-fill (best-effort). Query
        // just the event's own day rather than the full listing window.
        let meta_date = event.date.date();
        let meta = with_relogin(&client, || client.get_event_meta_on(event_id, meta_date))
            .await
            .ok()
            .flatten();
        let default_time = meta
            .as_ref()
            .and_then(|m| m.auto_open_input())
            .unwrap_or_default();
        let opens_hint = meta.and_then(|m| m.auto_open_date_time_display.clone());

        let error = q.error.as_deref().map(|code| {
            match code {
                "past" => "That time is in the past.",
                "badtime" => "Couldn't read that date/time.",
                "unknownclub" => "That club no longer exists.",
                "notschedulable" => {
                    "That slot can't be scheduled — it's full or doesn't accept member bookings."
                }
                _ => "Couldn't schedule that booking.",
            }
            .to_string()
        });

        Ok(render(&ScheduleTemplate {
            username: username_of(&auth),
            club_id,
            club_name: club.name,
            club_tz: club.timezone,
            event_id,
            group_id,
            event_title: event.name,
            event_date: event.date.format("%A %d %B %Y").to_string(),
            slot_time,
            slot_holes,
            slot_booked,
            slot_size,
            slot_full,
            default_time,
            opens_hint,
            error,
        })?
        .into_response())
    }
}

/// Fetch the events list for a club using its cached, logged-in client.
async fn fetch_events(state: &AppState, club: &Club) -> anyhow::Result<Vec<GolfEvent>> {
    let client = state.club_client(club).await?;
    with_relogin(&client, || client.get_events()).await
}

mod post {
    use super::*;

    pub async fn book(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path((club_id, event_id, group_id)): Path<(i64, u32, u32)>,
    ) -> Result<Response, AppError> {
        let club = load_club(&state, club_id).await?;
        let client = state.club_client(&club).await?;

        // Guard against booking a slot the club would reject anyway (full, or a
        // closed sheet). The page hides those buttons, but a stale page or a
        // hand-crafted POST must not slip through — and a dry run should report
        // the same refusal a real booking would hit, not a false "would book".
        let flash = match book_guard(&client, event_id, group_id).await {
            Some(text) => Some(Flash { ok: false, text }),
            None if state.config.dry_run => {
                tracing::info!(club = %club.name, group_id, "[DRY RUN] would book now");
                Some(Flash {
                    ok: true,
                    text: format!("Dry run — would book group {group_id} now (set DRY_RUN=false to book for real)."),
                })
            }
            None => Some(match client.book(group_id).await {
                Ok(()) => Flash {
                    ok: true,
                    text: "Booked.".to_string(),
                },
                Err(e) => Flash {
                    ok: false,
                    text: format!("Booking failed: {e}"),
                },
            }),
        };

        render_detail(
            username_of(&auth),
            club_id,
            club.name,
            &client,
            event_id,
            flash,
        )
        .await
    }

    /// Check whether a slot can actually be booked now. Returns `Some(reason)`
    /// when it can't (so the caller shows it as the failure), or `None` to
    /// proceed. A fetch failure is not a refusal — let the booking attempt be
    /// the source of truth rather than blocking on a transient read error.
    async fn book_guard(client: &GolfClient, event_id: u32, group_id: u32) -> Option<String> {
        let event = client.get_event(event_id).await.ok()?;
        match event.find_group(group_id) {
            Some(g) if g.is_full() => Some("That slot is already full.".to_string()),
            Some(g) if !g.accepts_members() => {
                Some("That slot doesn't accept member bookings.".to_string())
            }
            _ => None,
        }
    }
}
