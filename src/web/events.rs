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
    let event = client.get_event(event_id).await?;
    // Metadata is best-effort — a detail page is still useful without it.
    let summary = client
        .get_event_meta(event_id)
        .await
        .ok()
        .flatten()
        .map(|m| EventSummary::from(&m));

    Ok(render(&DetailTemplate {
        username,
        club_id,
        club_name,
        event,
        summary,
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
        let (events, error) = match fetch_events(&club).await {
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
        let client = GolfClient::from_club(&club);
        client.login().await?;
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
        let client = GolfClient::from_club(&club);
        client.login().await?;

        let event = client.get_event(event_id).await?;
        let group = event.find_group(group_id);
        let (slot_time, slot_holes, slot_booked) = match group {
            Some(g) => (Some(g.time.clone()), g.holes(), Some(g.entry_count())),
            None => (None, None, None),
        };

        // Metadata gives us the auto-open time to pre-fill (best-effort).
        let meta = client.get_event_meta(event_id).await.ok().flatten();
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
            default_time,
            opens_hint,
            error,
        })?
        .into_response())
    }
}

/// Build a client, log in, and fetch the events list for a club.
async fn fetch_events(club: &Club) -> anyhow::Result<Vec<GolfEvent>> {
    let client = GolfClient::from_club(club);
    client.login().await?;
    client.get_events().await
}

mod post {
    use super::*;

    pub async fn book(
        auth: AuthSession,
        State(state): State<Arc<AppState>>,
        Path((club_id, event_id, group_id)): Path<(i64, u32, u32)>,
    ) -> Result<Response, AppError> {
        let club = load_club(&state, club_id).await?;
        let client = GolfClient::from_club(&club);
        client.login().await?;

        let flash = if state.config.dry_run {
            tracing::info!(club = %club.name, group_id, "[DRY RUN] would book now");
            Flash {
                ok: true,
                text: format!("Dry run — would book group {group_id} now (set DRY_RUN=false to book for real)."),
            }
        } else {
            match client.book(group_id).await {
                Ok(()) => Flash {
                    ok: true,
                    text: "Booked.".to_string(),
                },
                Err(e) => Flash {
                    ok: false,
                    text: format!("Booking failed: {e}"),
                },
            }
        };

        render_detail(
            username_of(&auth),
            club_id,
            club.name,
            &client,
            event_id,
            Some(flash),
        )
        .await
    }
}
