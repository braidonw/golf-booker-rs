use chrono::NaiveDate;
use serde::Deserialize;

/// A single event in the club's events list (JSON from
/// `/spring/bookings/events/between/...`). Fields mirror the API; not all are
/// rendered yet.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct GolfEvent {
    #[serde(rename(deserialize = "bookingEventId"))]
    pub id: u32,
    pub event_date: NaiveDate,
    pub event_status_code: Option<u32>,
    pub event_status_code_friendly: Option<String>,
    pub title: String,
    pub booking_resource_id: Option<u32>,
    pub is_lottery: Option<bool>,
    pub can_open_event: Option<bool>,
    pub has_competition: Option<bool>,
    pub event_type_code: Option<u32>,
    pub event_category_code: Option<u32>,
    pub event_time_code_friendly: Option<String>,
    pub auto_open_date_time_display: Option<String>,
    pub availability: u32,
    pub is_ballot: bool,
    pub is_ballot_open: bool,
    pub is_results: bool,
    pub is_open: bool,
    pub is_female: bool,
    pub is_male: bool,
    pub is_matchplay: bool,
}

impl GolfEvent {
    /// A human-readable status, preferring the friendly string.
    pub fn status(&self) -> String {
        self.event_status_code_friendly
            .clone()
            .or_else(|| self.event_status_code.map(|c| c.to_string()))
            .unwrap_or_else(|| "Unknown".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_list() {
        let json = r#"[
            {"bookingEventId":101,"eventDate":"2026-06-20","title":"Saturday Comp",
             "availability":12,"isBallot":false,"isBallotOpen":false,"isResults":false,
             "isOpen":true,"isFemale":false,"isMale":true,"isMatchplay":false,
             "eventStatusCodeFriendly":"Open"}
        ]"#;
        let events: Vec<GolfEvent> = serde_json::from_str(json).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, 101);
        assert_eq!(events[0].title, "Saturday Comp");
        assert!(events[0].is_open);
        assert_eq!(events[0].status(), "Open");
    }

    #[test]
    fn status_falls_back_to_code_then_unknown() {
        let json = r#"[
            {"bookingEventId":1,"eventDate":"2026-06-20","title":"X","availability":0,
             "isBallot":false,"isBallotOpen":false,"isResults":false,"isOpen":false,
             "isFemale":false,"isMale":false,"isMatchplay":false,"eventStatusCode":7}
        ]"#;
        let events: Vec<GolfEvent> = serde_json::from_str(json).unwrap();
        assert_eq!(events[0].status(), "7");
    }
}
