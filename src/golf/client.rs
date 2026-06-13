//! HTTP client for a single golf club's MiClub-style booking API.
//!
//! Each client owns its own cookie jar, so clients for different clubs (or a
//! fresh client for the same club) keep independent sessions.

use super::{BookingEvent, GolfEvent};
use crate::clubs::Club;
use chrono::{Duration, Local};
use reqwest::Client;
use reqwest_cookie_store::CookieStoreMutex;
use std::sync::Arc;

// The cookie jar lives inside the reqwest `Client` (via its cookie provider);
// each `GolfClient` thus keeps its own session.

/// How far ahead to list events by default.
const DEFAULT_WINDOW_DAYS: i64 = 60;

#[derive(Clone)]
pub struct GolfClient {
    http: Client,
    base_url: String,
    username: String,
    password: String,
    member_id: String,
}

// Manual Debug so credentials never leak into logs.
impl std::fmt::Debug for GolfClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GolfClient")
            .field("base_url", &self.base_url)
            .field("username", &"[redacted]")
            .field("password", &"[redacted]")
            .field("member_id", &"[redacted]")
            .finish()
    }
}

impl GolfClient {
    pub fn new(base_url: &str, username: &str, password: &str, member_id: &str) -> Self {
        let cookies = Arc::new(CookieStoreMutex::default());
        let http = Client::builder()
            .cookie_provider(cookies)
            .build()
            .expect("reqwest client builds with default config");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
            member_id: member_id.to_string(),
        }
    }

    /// Build a client from a stored [`Club`] row.
    pub fn from_club(club: &Club) -> Self {
        Self::new(
            &club.base_url,
            &club.username,
            &club.password,
            &club.member_id,
        )
    }

    /// Authenticate, populating the cookie jar for subsequent requests.
    pub async fn login(&self) -> anyhow::Result<()> {
        let form = [
            ("user", self.username.as_str()),
            ("password", self.password.as_str()),
            ("action", "login"),
            ("Submit", "Login"),
        ];
        let url = format!("{}/security/login.msp", self.base_url);
        self.http.post(&url).form(&form).send().await?;
        Ok(())
    }

    /// List events from today to `today + DEFAULT_WINDOW_DAYS`.
    pub async fn get_events(&self) -> anyhow::Result<Vec<GolfEvent>> {
        let today = Local::now().date_naive();
        let to = today + Duration::days(DEFAULT_WINDOW_DAYS);
        self.get_events_between(today, to).await
    }

    /// List events between two dates (inclusive), formatted as the API expects.
    pub async fn get_events_between(
        &self,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> anyhow::Result<Vec<GolfEvent>> {
        let url = format!(
            "{}/spring/bookings/events/between/{}/{}/3000000",
            self.base_url,
            from.format("%d-%m-%Y"),
            to.format("%d-%m-%Y"),
        );
        let body = self.http.get(&url).send().await?.text().await?;
        let events = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("failed to parse events list: {e}"))?;
        Ok(events)
    }

    /// Fetch a single event with its booking sheet (XML).
    pub async fn get_event(&self, event_id: u32) -> anyhow::Result<BookingEvent> {
        let url = format!("{}/spring/bookings/events/{}", self.base_url, event_id);
        let body = self.http.get(&url).send().await?.text().await?;
        let event = quick_xml::de::from_str(&body)
            .map_err(|e| anyhow::anyhow!("failed to parse event {event_id}: {e}"))?;
        Ok(event)
    }

    /// Make a booking against a booking group (the `rowId`). Returns an error if
    /// the club responds with a booking error (e.g. sheet not open, already
    /// booked). Callers gate this behind dry-run where appropriate.
    pub async fn book(&self, booking_group_id: u32) -> anyhow::Result<()> {
        let row_id = booking_group_id.to_string();
        let params = [
            ("doAction", "makeBooking"),
            ("rowId", row_id.as_str()),
            ("memberId", self.member_id.as_str()),
            ("myGroup", "false"),
            ("findAlternative", "false"),
        ];
        let url = format!("{}/members/Ajax", self.base_url);
        let body = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await?
            .text()
            .await?;

        // A booking error comes back as an XML <Error><ErrorText>… document;
        // a success is anything that doesn't parse as that error shape.
        match parse_booking_error(&body) {
            Some(message) => Err(anyhow::anyhow!(message)),
            None => Ok(()),
        }
    }
}

/// Extract a booking error message from a response body, if it is one. Returns
/// `None` when the body isn't an error document (or carries no error text),
/// which we treat as a successful booking.
fn parse_booking_error(body: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ErrorResponse {
        #[serde(rename = "Error", default)]
        errors: Vec<ErrorItem>,
    }
    #[derive(serde::Deserialize)]
    struct ErrorItem {
        #[serde(rename = "ErrorText")]
        text: String,
    }

    let parsed: ErrorResponse = quick_xml::de::from_str(body).ok()?;
    if parsed.errors.is_empty() {
        return None;
    }
    let message = parsed
        .errors
        .into_iter()
        .map(|e| e.text)
        .collect::<Vec<_>>()
        .join(", ");
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_booking_error() {
        let xml = "<Response><Error><ErrorText>Sheet not open yet</ErrorText></Error></Response>";
        assert_eq!(
            parse_booking_error(xml).as_deref(),
            Some("Sheet not open yet")
        );
    }

    #[test]
    fn joins_multiple_errors() {
        let xml = "<Response><Error><ErrorText>A</ErrorText></Error>\
                   <Error><ErrorText>B</ErrorText></Error></Response>";
        assert_eq!(parse_booking_error(xml).as_deref(), Some("A, B"));
    }

    #[test]
    fn no_error_elements_is_success() {
        // A response with no <Error> elements is a successful booking.
        assert!(parse_booking_error("<Response><ok/></Response>").is_none());
    }
}
