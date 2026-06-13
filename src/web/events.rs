//! Multi-club event browsing and immediate ("book now") booking.

use super::app::AppState;
use crate::clubs::Club;
use crate::error::AppError;
use crate::golf::{BookingEvent, GolfClient, GolfEvent};
use crate::users::AuthSession;
use crate::web::render;
use askama::Template;
use axum::{
    extract::{Path, State},
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

/// Event detail with its booking sheet. Rendered by both the GET view and the
/// POST book handler (the latter adds a `flash`), so it lives at module scope.
#[derive(Template)]
#[template(path = "events/detail.html")]
struct DetailTemplate {
    username: String,
    club_id: i64,
    club_name: String,
    event: BookingEvent,
    flash: Option<Flash>,
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
        let event = client.get_event(event_id).await?;

        Ok(render(&DetailTemplate {
            username: username_of(&auth),
            club_id,
            club_name: club.name,
            event,
            flash: None,
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

        // Re-fetch so the sheet reflects the (attempted) booking.
        let event = client.get_event(event_id).await?;

        Ok(render(&DetailTemplate {
            username: username_of(&auth),
            club_id,
            club_name: club.name,
            event,
            flash: Some(flash),
        })?
        .into_response())
    }
}
