//! Deserialization types mirroring the club's MiClub-style API.
//!
//! These mirror an external API shape we don't control, so some fields are
//! parsed but not yet read — `#[allow(dead_code)]` is intentional here.

mod booking_event;
mod booking_group;
mod event;

pub use booking_event::BookingEvent;
pub use event::GolfEvent;
