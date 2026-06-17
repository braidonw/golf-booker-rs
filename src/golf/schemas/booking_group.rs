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
    /// Capacity of the slot (the `size` XML attribute), e.g. 4 players. Older
    /// sheets that omit it fall back to the standard fourball.
    #[serde(rename(deserialize = "@size"), default = "default_size")]
    pub size: u32,
    #[serde(rename(deserialize = "lastModified"))]
    pub last_modified: String,
    #[serde(rename(deserialize = "lastModifierId"))]
    pub last_modifier_id: u32,
    /// Whether this slot is open for booking right now. The whole sheet (event
    /// → section → group) reads `false` while the tee sheet is closed; the
    /// club site renders those read-only.
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

/// The standard tee-group capacity, used when a sheet omits the `size`
/// attribute (real MiClub sheets always carry it).
fn default_size() -> u32 {
    4
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

    /// No free places left — every seat in the group is taken.
    pub fn is_full(&self) -> bool {
        self.entry_count() >= self.size as usize
    }

    /// Whether members may book this slot at all. We always book as a member
    /// (via the club's stored `member_id`), so a slot that doesn't accept
    /// members is never bookable for us regardless of free seats.
    pub fn accepts_members(&self) -> bool {
        self.member_accepted
    }

    /// Has a free seat we could take: accepts members and isn't full. Whether
    /// we *book it now* or *schedule it* depends on the event-level `is_open`
    /// flag, decided by the caller — the per-group `active` flag is unreliable
    /// (it reads `false` even on sheets that are plainly open and being booked).
    pub fn is_schedulable(&self) -> bool {
        self.accepts_members() && !self.is_full()
    }

    /// Human-readable eligibility rules for the slot, for display. These mirror
    /// the club's own requirements so a member can see what a slot demands
    /// before booking (we can't fully pre-check them without member profile
    /// data, so they're informational rather than gates).
    pub fn requirement_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.visitor_accepted {
            labels.push("Visitors OK");
        }
        if self.require_golf_link {
            labels.push("GolfLink required");
        }
        if self.require_handicap {
            labels.push("Handicap required");
        }
        if self.require_home_club {
            labels.push("Home club required");
        }
        if self.require_gender {
            labels.push("Gender required");
        }
        labels
    }

    /// Provisional category from the MiClub `StatusCode`: `3070` reads as a
    /// competition slot and `3071` as casual/social across the sheets we've
    /// seen. This mapping is **unconfirmed** — surfaced in the UI to gather
    /// user feedback before anything depends on it. `None` for other codes.
    pub fn category_label(&self) -> Option<&'static str> {
        match self.status_code {
            3070 => Some("Competition"),
            3071 => Some("Casual"),
            _ => None,
        }
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
    /// The player's home club — present for visitors/guests from another club,
    /// absent for the host club's own members.
    #[serde(rename(deserialize = "HomeClub"))]
    pub home_club: Option<String>,
    #[serde(rename(deserialize = "Gender"))]
    pub gender: Option<String>,
    #[serde(rename(deserialize = "Handicap"))]
    pub handicap: Option<f32>,
    #[serde(rename(deserialize = "GolfLinkNo"))]
    pub golf_link_no: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_with(entries: usize, size: u32) -> BookingGroup {
        BookingGroup {
            size,
            member_accepted: true,
            booking_entries: BookingEntries {
                entries: (0..entries).map(|_| BookingEntry::default()).collect(),
            },
            ..BookingGroup::default()
        }
    }

    #[test]
    fn holes_prefers_nine_then_eighteen_then_none() {
        let nine = BookingGroup {
            nine_holes: true,
            eighteen_holes: true,
            ..BookingGroup::default()
        };
        assert_eq!(nine.holes(), Some(9));
        let eighteen = BookingGroup {
            eighteen_holes: true,
            ..BookingGroup::default()
        };
        assert_eq!(eighteen.holes(), Some(18));
        assert_eq!(BookingGroup::default().holes(), None);
    }

    #[test]
    fn is_full_is_a_seat_count_boundary() {
        assert!(!group_with(3, 4).is_full());
        assert!(group_with(4, 4).is_full());
        // Over-subscribed (defensive): still full.
        assert!(group_with(5, 4).is_full());
    }

    #[test]
    fn schedulable_requires_member_access_and_a_free_seat() {
        assert!(group_with(2, 4).is_schedulable());
        // Full slot: not schedulable.
        assert!(!group_with(4, 4).is_schedulable());
        // Member-closed slot: not schedulable even with seats free.
        let closed = BookingGroup {
            member_accepted: false,
            ..group_with(0, 4)
        };
        assert!(!closed.is_schedulable());
    }

    #[test]
    fn category_label_maps_known_codes_only() {
        let cat = |c| {
            BookingGroup {
                status_code: c,
                ..BookingGroup::default()
            }
            .category_label()
        };
        assert_eq!(cat(3070), Some("Competition"));
        assert_eq!(cat(3071), Some("Casual"));
        assert_eq!(cat(9999), None);
    }

    #[test]
    fn requirement_labels_are_empty_when_nothing_required() {
        assert!(BookingGroup::default().requirement_labels().is_empty());
    }
}
