use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct BookingGroups {
    #[serde(rename(deserialize = "BookingGroup"))]
    pub groups: Option<Vec<BookingGroup>>,
}

/// A bookable tee slot within a section. Fields mirror the API XML.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
pub struct BookingGroup {
    pub id: u32,
    #[serde(rename(deserialize = "lastModified"))]
    pub last_modified: String,
    #[serde(rename(deserialize = "lastModifierId"))]
    pub last_modifier_id: u32,
    pub active: bool,
    #[serde(rename(deserialize = "Time"))]
    pub time: String,
    #[serde(rename(deserialize = "StatusCode"))]
    pub status_code: u32,
    #[serde(rename(deserialize = "RequireGender"))]
    pub require_gender: bool,
    #[serde(rename(deserialize = "RequireGolfLink"))]
    pub require_golf_link: bool,
    #[serde(rename(deserialize = "RequireHandicap"))]
    pub require_handicap: bool,
    #[serde(rename(deserialize = "RequireHomeClub"))]
    pub require_home_club: bool,
    #[serde(rename(deserialize = "VisitorAccepted"))]
    pub visitor_accepted: bool,
    #[serde(rename(deserialize = "MemberAccepted"))]
    pub member_accepted: bool,
    #[serde(rename(deserialize = "PublicMemberAccepted"))]
    pub public_member_accepted: bool,
    #[serde(rename(deserialize = "NineHoles"))]
    pub nine_holes: bool,
    #[serde(rename(deserialize = "EighteenHoles"))]
    pub eighteen_holes: bool,
    #[serde(rename(deserialize = "BookingEntries"))]
    pub booking_entries: BookingEntries,
}

impl BookingGroup {
    /// 9 or 18 holes, if specified.
    pub fn holes(&self) -> Option<u32> {
        if self.nine_holes {
            Some(9)
        } else if self.eighteen_holes {
            Some(18)
        } else {
            None
        }
    }

    /// People currently in the slot.
    pub fn entry_count(&self) -> usize {
        self.booking_entries.entries.len()
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct BookingEntries {
    #[serde(rename(deserialize = "BookingEntry"), default)]
    pub entries: Vec<BookingEntry>,
}

/// A person occupying a slot. Fields mirror the API XML.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
pub struct BookingEntry {
    #[serde(rename(deserialize = "@id"))]
    pub id: u32,
    #[serde(rename(deserialize = "@type"))]
    pub kind: String,
    #[serde(rename(deserialize = "@index"))]
    pub index: u32,
    #[serde(rename(deserialize = "PersonName"))]
    pub person_name: String,
    #[serde(rename(deserialize = "MembershipNumber"))]
    pub membership_number: Option<String>,
    #[serde(rename(deserialize = "Gender"))]
    pub gender: Option<String>,
    #[serde(rename(deserialize = "Handicap"))]
    pub handicap: Option<f32>,
    #[serde(rename(deserialize = "GolfLinkNo"))]
    pub golf_link_no: Option<String>,
}
