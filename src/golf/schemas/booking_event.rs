use super::booking_group::BookingGroups;
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
    #[serde(rename(deserialize = "lastModified"))]
    pub last_modified: String,
    #[serde(rename(deserialize = "lastModifierId"))]
    pub last_modifier_id: Option<u32>,
    #[serde(rename(deserialize = "BookingSections"))]
    pub booking_sections: BookingSections,
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
    #[serde(rename(deserialize = "BookingGroups"))]
    pub booking_groups: BookingGroups,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the real MiClub shape: `id`/`active`/`Date` are child *elements*,
    // while booking-entry `@id`/`@type`/`@index` are attributes.
    const SAMPLE: &str = r#"<?xml version="1.0"?>
<BookingEvent>
  <active>true</active>
  <id>101</id>
  <Date>2026-06-20T07:00:00</Date>
  <Name>Saturday Comp</Name>
  <lastModified>2026-06-01</lastModified>
  <lastModifierId>1</lastModifierId>
  <BookingSections>
    <BookingSection id="1">
      <lastModified>2026-06-01</lastModified>
      <lastModifierId>1</lastModifierId>
      <active>true</active>
      <Name>Front Nine</Name>
      <BookingGroups>
        <BookingGroup>
          <id>5001</id>
          <lastModified>2026-06-01</lastModified>
          <lastModifierId>1</lastModifierId>
          <active>true</active>
          <Time>07:00</Time>
          <StatusCode>0</StatusCode>
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
          </BookingEntries>
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

        let section = &event.booking_sections.sections[0];
        assert_eq!(section.name, "Front Nine");

        let group = &section.booking_groups.groups.as_ref().unwrap()[0];
        assert_eq!(group.id, 5001);
        assert_eq!(group.time, "07:00");
        assert_eq!(group.holes(), Some(18));
        assert_eq!(group.entry_count(), 1);
        assert_eq!(group.booking_entries.entries[0].person_name, "J. Smith");
    }
}
