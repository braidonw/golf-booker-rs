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

    /// Who the event accepts, from the male/female flags.
    pub fn gender_label(&self) -> &'static str {
        match (self.is_male, self.is_female) {
            (true, true) => "Men & Women",
            (true, false) => "Men",
            (false, true) => "Women",
            (false, false) => "Open",
        }
    }

    /// Parse `auto_open_date_time_display` (the club-local time the sheet opens)
    /// into a naive datetime, trying the formats MiClub is known to use. Returns
    /// `None` if absent or unparseable.
    pub fn auto_open_local(&self) -> Option<chrono::NaiveDateTime> {
        let raw = self.auto_open_date_time_display.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        const FORMATS: &[&str] = &[
            "%d/%m/%Y %H:%M",
            "%d/%m/%Y %I:%M %p",
            "%d/%m/%Y %I:%M%p",
            "%d/%m/%Y %l:%M %p",
            "%d-%m-%Y %H:%M",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M",
            "%a %d %b %Y %I:%M %p",
            "%A %d %B %Y %I:%M %p",
            "%d %b %Y %I:%M %p",
            "%d %B %Y %I:%M %p",
            "%d %b %Y %H:%M",
        ];
        for fmt in FORMATS {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
                return Some(dt);
            }
        }
        None
    }

    /// `auto_open_local` formatted for a `datetime-local` input, if parseable.
    pub fn auto_open_input(&self) -> Option<String> {
        self.auto_open_local()
            .map(|dt| dt.format("%Y-%m-%dT%H:%M").to_string())
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
    fn parses_auto_open_display_formats() {
        let mk = |s: &str| GolfEvent {
            id: 1,
            event_date: "2026-06-20".parse().unwrap(),
            event_status_code: None,
            event_status_code_friendly: None,
            title: "x".into(),
            booking_resource_id: None,
            is_lottery: None,
            can_open_event: None,
            has_competition: None,
            event_type_code: None,
            event_category_code: None,
            event_time_code_friendly: None,
            auto_open_date_time_display: Some(s.into()),
            availability: 0,
            is_ballot: false,
            is_ballot_open: false,
            is_results: false,
            is_open: false,
            is_female: false,
            is_male: false,
            is_matchplay: false,
        };
        assert_eq!(
            mk("11/06/2026 07:00").auto_open_input().as_deref(),
            Some("2026-06-11T07:00")
        );
        assert_eq!(
            mk("11/06/2026 7:00 AM").auto_open_input().as_deref(),
            Some("2026-06-11T07:00")
        );
        assert!(mk("sometime next week").auto_open_input().is_none());
        assert!(mk("").auto_open_input().is_none());
    }

    #[test]
    fn gender_label_reflects_flags() {
        let mut e: GolfEvent = serde_json::from_str(
            r#"{"bookingEventId":1,"eventDate":"2026-06-20","title":"x","availability":0,"isBallot":false,"isBallotOpen":false,"isResults":false,"isOpen":true,"isFemale":false,"isMale":true,"isMatchplay":false}"#,
        ).unwrap();
        assert_eq!(e.gender_label(), "Men");
        e.is_female = true;
        assert_eq!(e.gender_label(), "Men & Women");
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
