use super::booking_group::{BookingGroup, BookingGroups};
use chrono::NaiveDateTime;
use serde::Deserialize;

/// A single event with its booking sheet (XML from
/// `/spring/bookings/events/{id}`).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BookingEvent {
    pub active: bool,
    pub id: u32,
    #[serde(rename(deserialize = "Date"))]
    pub date: NaiveDateTime,
    #[serde(rename(deserialize = "Name"))]
    pub name: String,
    /// Who the event is open to: "All", "Male", "Female".
    #[serde(rename(deserialize = "Gender"), default)]
    pub gender: Option<String>,
    #[serde(rename(deserialize = "lastModified"))]
    pub last_modified: String,
    #[serde(rename(deserialize = "lastModifierId"))]
    pub last_modifier_id: Option<u32>,
    #[serde(rename(deserialize = "BookingSections"))]
    pub booking_sections: BookingSections,
}

impl BookingEvent {
    /// Find a booking group anywhere in the sheet by its id.
    pub fn find_group(&self, id: u32) -> Option<&BookingGroup> {
        self.booking_sections
            .sections
            .iter()
            .flat_map(|s| s.booking_groups.groups.iter().flatten())
            .find(|g| g.id == id)
    }
}

#[derive(Debug, Deserialize)]
pub struct BookingSections {
    #[serde(rename(deserialize = "BookingSection"))]
    pub sections: Vec<BookingSection>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BookingSection {
    #[serde(rename(deserialize = "lastModified"))]
    pub last_modified: String,
    #[serde(rename(deserialize = "lastModifierId"))]
    pub last_modifier_id: u32,
    #[serde(rename(deserialize = "@id"))]
    pub id: u32,
    pub active: bool,
    #[serde(rename(deserialize = "Name"))]
    pub name: String,
    /// Display order of the section within the sheet (the `Index` element).
    #[serde(rename(deserialize = "Index"), default)]
    pub index: Option<u32>,
    #[serde(rename(deserialize = "BookingGroups"))]
    pub booking_groups: BookingGroups,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the real MiClub shape: `id`/`active`/`Date` are child *elements*,
    // while booking-entry `@id`/`@type`/`@index` are attributes.
    const SAMPLE: &str = r#"<?xml version="1.0"?>
<BookingEvent id="101">
  <active>false</active>
  <id>101</id>
  <Date>2026-06-20T07:00:00</Date>
  <Name>Saturday Comp</Name>
  <Gender>All</Gender>
  <lastModified>2026-06-01</lastModified>
  <lastModifierId>1</lastModifierId>
  <BookingSections>
    <BookingSection id="1">
      <lastModified>2026-06-01</lastModified>
      <lastModifierId>1</lastModifierId>
      <active>false</active>
      <Name>Front Nine</Name>
      <Index>1</Index>
      <BookingGroups>
        <BookingGroup id="5001" size="4">
          <id>5001</id>
          <lastModified>2026-06-01</lastModified>
          <lastModifierId>1</lastModifierId>
          <active>false</active>
          <Time>07:00</Time>
          <StatusCode>3070</StatusCode>
          <RequireGender>false</RequireGender>
          <RequireGolfLink>false</RequireGolfLink>
          <RequireHandicap>false</RequireHandicap>
          <RequireHomeClub>false</RequireHomeClub>
          <VisitorAccepted>false</VisitorAccepted>
          <MemberAccepted>true</MemberAccepted>
          <PublicMemberAccepted>false</PublicMemberAccepted>
          <NineHoles>false</NineHoles>
          <EighteenHoles>true</EighteenHoles>
          <BookingEntries>
            <BookingEntry id="1" type="member" index="0">
              <PersonName>J. Smith</PersonName>
            </BookingEntry>
            <BookingEntry id="2" type="member" index="1">
              <PersonName>A. Jones</PersonName>
            </BookingEntry>
            <BookingEntry id="3" type="member" index="2">
              <PersonName>B. Brown</PersonName>
            </BookingEntry>
            <BookingEntry id="4" type="member" index="3">
              <PersonName>C. Green</PersonName>
            </BookingEntry>
          </BookingEntries>
        </BookingGroup>
        <BookingGroup id="5002" size="4">
          <id>5002</id>
          <lastModified>2026-06-01</lastModified>
          <lastModifierId>1</lastModifierId>
          <active>true</active>
          <Time>07:08</Time>
          <StatusCode>3071</StatusCode>
          <RequireGender>false</RequireGender>
          <RequireGolfLink>false</RequireGolfLink>
          <RequireHandicap>false</RequireHandicap>
          <RequireHomeClub>false</RequireHomeClub>
          <VisitorAccepted>false</VisitorAccepted>
          <MemberAccepted>true</MemberAccepted>
          <PublicMemberAccepted>false</PublicMemberAccepted>
          <NineHoles>false</NineHoles>
          <EighteenHoles>true</EighteenHoles>
          <BookingEntries/>
        </BookingGroup>
      </BookingGroups>
    </BookingSection>
  </BookingSections>
</BookingEvent>"#;

    #[test]
    fn parses_event_sheet() {
        let event: BookingEvent = quick_xml::de::from_str(SAMPLE).unwrap();
        assert_eq!(event.id, 101);
        assert_eq!(event.name, "Saturday Comp");
        assert_eq!(event.gender.as_deref(), Some("All"));

        let section = &event.booking_sections.sections[0];
        assert_eq!(section.name, "Front Nine");
        assert_eq!(section.index, Some(1));

        let groups = section.booking_groups.groups.as_ref().unwrap();
        let full = &groups[0];
        assert_eq!(full.id, 5001);
        assert_eq!(full.time, "07:00");
        assert_eq!(full.size, 4);
        assert_eq!(full.holes(), Some(18));
        assert_eq!(full.entry_count(), 4);
        assert_eq!(full.booking_entries.entries[0].person_name, "J. Smith");
    }

    #[test]
    fn full_closed_slot_is_neither_bookable_nor_schedulable() {
        let event: BookingEvent = quick_xml::de::from_str(SAMPLE).unwrap();
        let groups = event.booking_sections.sections[0]
            .booking_groups
            .groups
            .as_ref()
            .unwrap();

        // 4/4 and the sheet is closed: not bookable now, and full means you
        // can't schedule against it either.
        let full = &groups[0];
        assert!(full.is_full());
        assert!(!full.is_bookable_now());
        assert!(!full.is_schedulable());
    }

    #[test]
    fn open_empty_slot_is_bookable_and_schedulable() {
        let event: BookingEvent = quick_xml::de::from_str(SAMPLE).unwrap();
        let groups = event.booking_sections.sections[0]
            .booking_groups
            .groups
            .as_ref()
            .unwrap();

        // Empty seat on an active sheet: bookable now and schedulable.
        let open = &groups[1];
        assert!(!open.is_full());
        assert!(open.is_bookable_now());
        assert!(open.is_schedulable());
    }

    #[test]
    fn member_closed_slot_is_not_bookable_even_when_empty() {
        // A slot that doesn't accept members can't be booked by us (we book as
        // a member), no matter how many seats are free or whether it's active.
        let xml = r#"<BookingGroup id="1" size="4">
            <id>1</id><lastModified>x</lastModified><lastModifierId>1</lastModifierId>
            <active>true</active><Time>07:00</Time><StatusCode>3071</StatusCode>
            <RequireGender>false</RequireGender><RequireGolfLink>false</RequireGolfLink>
            <RequireHandicap>false</RequireHandicap><RequireHomeClub>false</RequireHomeClub>
            <VisitorAccepted>true</VisitorAccepted><MemberAccepted>false</MemberAccepted>
            <PublicMemberAccepted>true</PublicMemberAccepted>
            <NineHoles>false</NineHoles><EighteenHoles>true</EighteenHoles>
            <BookingEntries/>
        </BookingGroup>"#;
        let group: BookingGroup = quick_xml::de::from_str(xml).unwrap();
        assert!(!group.is_full());
        assert!(!group.accepts_members());
        assert!(!group.is_bookable_now());
        assert!(!group.is_schedulable());
    }

    #[test]
    fn surfaces_category_and_requirement_labels() {
        let xml = r#"<BookingGroup id="1" size="4">
            <id>1</id><lastModified>x</lastModified><lastModifierId>1</lastModifierId>
            <active>true</active><Time>10:06</Time><StatusCode>3070</StatusCode>
            <RequireGender>true</RequireGender><RequireGolfLink>true</RequireGolfLink>
            <RequireHandicap>false</RequireHandicap><RequireHomeClub>false</RequireHomeClub>
            <VisitorAccepted>true</VisitorAccepted><MemberAccepted>true</MemberAccepted>
            <PublicMemberAccepted>false</PublicMemberAccepted>
            <NineHoles>false</NineHoles><EighteenHoles>true</EighteenHoles>
            <BookingEntries/>
        </BookingGroup>"#;
        let group: BookingGroup = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(group.category_label(), Some("Competition"));
        assert_eq!(
            group.requirement_labels(),
            vec!["Visitors OK", "GolfLink required", "Gender required"]
        );
    }

    #[test]
    fn size_defaults_to_a_fourball_when_absent() {
        // An older sheet without the `size` attribute still parses, defaulting
        // to the standard fourball.
        let xml = r#"<BookingGroup>
            <id>1</id><lastModified>x</lastModified><lastModifierId>1</lastModifierId>
            <active>true</active><Time>07:00</Time><StatusCode>3071</StatusCode>
            <RequireGender>false</RequireGender><RequireGolfLink>false</RequireGolfLink>
            <RequireHandicap>false</RequireHandicap><RequireHomeClub>false</RequireHomeClub>
            <VisitorAccepted>false</VisitorAccepted><MemberAccepted>true</MemberAccepted>
            <PublicMemberAccepted>false</PublicMemberAccepted>
            <NineHoles>false</NineHoles><EighteenHoles>true</EighteenHoles>
            <BookingEntries/>
        </BookingGroup>"#;
        let group: BookingGroup = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(group.size, 4);
    }
}
