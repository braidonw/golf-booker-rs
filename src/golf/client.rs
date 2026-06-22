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
            // Without timeouts a stalled connection blocks indefinitely. The
            // booking POST sets its own tighter per-request timeout (the race
            // window is only seconds); these are the outer safety net shared by
            // login and the web-facing event fetches.
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(15))
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
    ///
    /// `error_for_status` turns a 4xx/5xx (bad credentials, server error,
    /// maintenance page) into an `Err` — reqwest treats those as success
    /// otherwise. A club that re-renders the login page with HTTP 200 on a bad
    /// password can't be caught here without a known page marker; the booking
    /// path's positive-response check (`book`) is the backstop for that.
    pub async fn login(&self) -> anyhow::Result<()> {
        let form = [
            ("user", self.username.as_str()),
            ("password", self.password.as_str()),
            ("action", "login"),
            ("Submit", "Login"),
        ];
        let url = format!("{}/security/login.msp", self.base_url);
        self.http
            .post(&url)
            .form(&form)
            .send()
            .await?
            .error_for_status()?;
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

    /// Fetch the list metadata for one event (category, type, gender, auto-open
    /// time, …) by querying only its own day, rather than scanning the full
    /// default window. Callers already hold the event's booking sheet (from
    /// [`get_event`]), which carries its date. `None` if the event isn't in that
    /// day's listing.
    pub async fn get_event_meta_on(
        &self,
        event_id: u32,
        date: chrono::NaiveDate,
    ) -> anyhow::Result<Option<GolfEvent>> {
        Ok(self
            .get_events_between(date, date)
            .await?
            .into_iter()
            .find(|e| e.id == event_id))
    }

    /// Fetch a single event with its booking sheet (XML).
    pub async fn get_event(&self, event_id: u32) -> anyhow::Result<BookingEvent> {
        let url = format!("{}/spring/bookings/events/{}", self.base_url, event_id);
        let body = self.http.get(&url).send().await?.text().await?;
        let event = quick_xml::de::from_str(&body)
            .map_err(|e| anyhow::anyhow!("failed to parse event {event_id}: {e}"))?;
        Ok(event)
    }

    /// Make a booking against a booking group (the `rowId`).
    ///
    /// Errors are classified so the scheduler can keep racing on retryable
    /// failures ("sheet not open yet", transient network) but stop immediately
    /// on terminal ones ("already booked", "not eligible").
    pub async fn book(&self, booking_group_id: u32) -> Result<(), BookingError> {
        let row_id = booking_group_id.to_string();
        let params = [
            ("doAction", "makeBooking"),
            ("rowId", row_id.as_str()),
            ("memberId", self.member_id.as_str()),
            ("myGroup", "true"),
            ("findAlternative", "false"),
        ];
        let url = format!("{}/members/Ajax", self.base_url);

        // A booking attempt must fail fast: the retry loop only re-checks its
        // deadline between attempts, so a stalled request would otherwise eat
        // the whole race window. This timeout is well under that window.
        let resp = self
            .http
            .post(&url)
            .form(&params)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            // Transport failures (incl. timeout) are worth retrying mid-race.
            .map_err(|e| BookingError::Retryable(format!("request failed: {e}")))?;

        // A non-2xx (auth lapsed since pre-auth, 5xx) is not a booking — retry
        // rather than fall through to the "no error document == success" path.
        let resp = resp.error_for_status().map_err(|e| {
            BookingError::Retryable(format!("booking request returned error status: {e}"))
        })?;
        let body = resp
            .text()
            .await
            .map_err(|e| BookingError::Retryable(format!("reading response failed: {e}")))?;

        // We infer success from the *absence* of an error document (see
        // `interpret_booking_response`), because the club's success body isn't
        // pinned down. Log a truncated copy so the first real `DRY_RUN=false`
        // booking captures what success actually looks like, and we can later
        // assert a positive marker. Off by default (debug level).
        tracing::debug!(response = %truncate(&body, 600), "booking response body");

        interpret_booking_response(&body)
    }
}

/// Shorten a response body for logging without dumping an entire HTML page.
/// Counts characters (not bytes) so it never splits a UTF-8 boundary.
fn truncate(s: &str, max_chars: usize) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= max_chars {
        std::borrow::Cow::Borrowed(s)
    } else {
        let head: String = s.chars().take(max_chars).collect();
        std::borrow::Cow::Owned(format!("{head}… ({} bytes total)", s.len()))
    }
}

/// A booking failure, tagged with whether retrying could plausibly succeed.
#[derive(Debug)]
pub enum BookingError {
    /// Worth retrying within the race window (sheet not open yet, transient).
    Retryable(String),
    /// Permanent — retrying won't help (already booked, ineligible).
    Terminal(String),
}

impl BookingError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, BookingError::Retryable(_))
    }

    fn message(&self) -> &str {
        match self {
            BookingError::Retryable(m) | BookingError::Terminal(m) => m,
        }
    }
}

impl std::fmt::Display for BookingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for BookingError {}

/// Classify a club booking-error message as retryable or terminal. Unknown
/// messages default to retryable, since during the race "not open yet" is the
/// common case and retrying is cheap; the retry window bounds the cost.
fn classify_booking_error(message: &str) -> BookingError {
    // Booking-specific phrases, not bare ambiguous words. Misclassifying a
    // transient message as terminal aborts a winnable race, so the markers lean
    // specific: an unmatched *real* terminal error merely wastes a few cheap
    // retries before failing, which is the safe direction to err.
    const TERMINAL_MARKERS: &[&str] = &[
        "already booked",
        "already have",
        "not eligible",
        "ineligible",
        "not allowed",
        "not permitted",
        "duplicate",
        "booking limit",
        "exceeded the maximum",
        "maximum number",
        "no longer available",
        "permission",
    ];
    let lower = message.to_lowercase();
    if TERMINAL_MARKERS.iter().any(|m| lower.contains(m)) {
        BookingError::Terminal(message.to_string())
    } else {
        BookingError::Retryable(message.to_string())
    }
}

/// Decide what a booking response body means.
///
/// MiClub signals a booking failure with an XML `<Error><ErrorText>…` document.
/// We can't require a *positive* success shape (the club's success body isn't
/// pinned down), so absence of an error document is treated as booked — except
/// when the body is plainly a login/HTML page, which means the session lapsed
/// between pre-auth and firing. Treating that as success would be a false
/// booking, so it's retried instead.
fn interpret_booking_response(body: &str) -> Result<(), BookingError> {
    if let Some(message) = parse_booking_error(body) {
        return Err(classify_booking_error(&message));
    }
    if looks_like_login_page(body) {
        return Err(BookingError::Retryable(
            "booking returned a login/HTML page — session likely expired".to_string(),
        ));
    }
    // An empty/whitespace body is not a confirmed booking. MiClub signals both
    // success and failure with an XML document, so a blank response is far more
    // likely a dropped or proxied request than a real booking — retry rather
    // than record a booking that may not have happened. (If a live smoke test
    // shows success really is an empty 200, revisit this.)
    if body.trim().is_empty() {
        return Err(BookingError::Retryable(
            "booking returned an empty response".to_string(),
        ));
    }
    Ok(())
}

/// Heuristic: does this body look like a login redirect or HTML error page
/// rather than an API response? Guards against recording a false booking when
/// the club bounces us to login.
fn looks_like_login_page(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("<html")
        || lower.contains("<!doctype")
        // MiClub's login form, and Spring Security's default login endpoint.
        || lower.contains("login.msp")
        || lower.contains("j_security_check")
}

/// Extract a booking error message from a response body, if it is one. Returns
/// `None` when the body isn't an error document (or carries no error text).
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

    #[test]
    fn interprets_clean_response_as_booked() {
        assert!(interpret_booking_response("<Response><ok/></Response>").is_ok());
    }

    #[test]
    fn interprets_error_document_as_failure() {
        let xml = "<Response><Error><ErrorText>Member not eligible</ErrorText></Error></Response>";
        let err = interpret_booking_response(xml).unwrap_err();
        assert!(!err.is_retryable(), "eligibility error should be terminal");
    }

    #[test]
    fn login_page_response_is_retryable_not_booked() {
        // A lapsed session bounces us to a login/HTML page. Treating that as a
        // success would record a booking that never happened.
        for body in [
            "<!DOCTYPE html><html><body>Please log in</body></html>",
            "<html><form action=\"/security/login.msp\"></form></html>",
        ] {
            let err = interpret_booking_response(body)
                .expect_err("login page must not count as a booking");
            assert!(err.is_retryable(), "expected retryable for: {body}");
        }
    }

    #[test]
    fn classifies_terminal_errors() {
        for msg in [
            "You have already booked this competition",
            "Member not eligible for this event",
            "Booking limit exceeded",
        ] {
            assert!(
                !classify_booking_error(msg).is_retryable(),
                "expected terminal: {msg}"
            );
        }
    }

    #[test]
    fn classifies_retryable_and_unknown_errors() {
        for msg in ["Booking sheet is not open yet", "Some unexpected message"] {
            assert!(
                classify_booking_error(msg).is_retryable(),
                "expected retryable: {msg}"
            );
        }
    }

    #[test]
    fn empty_response_is_retryable_not_booked() {
        // A blank body is not a confirmed booking — treating it as success could
        // record a booking that never happened.
        for body in ["", "   ", "\n\t  \n"] {
            let err = interpret_booking_response(body)
                .expect_err("empty body must not count as a booking");
            assert!(err.is_retryable(), "expected retryable for {body:?}");
        }
    }

    #[test]
    fn detects_spring_security_login_redirect() {
        let body = "<html><form action=\"/j_security_check\" method=\"post\"></form></html>";
        let err = interpret_booking_response(body).expect_err("login redirect is not a booking");
        assert!(err.is_retryable());
    }

    #[test]
    fn truncate_is_char_safe_and_bounded() {
        assert_eq!(truncate("short", 600), "short");
        // Multi-byte characters must not be split mid-codepoint.
        let long = "é".repeat(1000);
        let out = truncate(&long, 10);
        assert!(out.starts_with(&"é".repeat(10)));
        assert!(out.contains("bytes total"));
    }

    #[test]
    fn ambiguous_words_alone_are_not_terminal() {
        // These contain words that used to trip the terminal markers (maximum,
        // exceeded, no longer) but are plausibly transient — they must stay
        // retryable so a winnable race isn't abandoned.
        for msg in [
            "Server at maximum capacity — please retry",
            "Gateway timeout exceeded",
            "Slot is no longer shown but may reopen shortly",
        ] {
            assert!(
                classify_booking_error(msg).is_retryable(),
                "expected retryable: {msg}"
            );
        }
    }
}
