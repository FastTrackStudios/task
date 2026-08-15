//! Booking frontmatter → `Booking`.

use scheduling_proto::{Booking, BookingId, BookingStatus, EventTypeId};

use super::ParseError;
use super::yaml::{parse_mapping, require_str, take_str};

pub fn parse_booking(path: &str, frontmatter_yaml: &str) -> Result<Booking, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;

    let id = take_str(&map, "id").unwrap_or_else(|| path.to_string());
    let event_type_id = require_str(&map, "event_type_id")?;
    let start_utc = require_str(&map, "start_utc")?;
    let end_utc = require_str(&map, "end_utc")?;
    let attendee_name = require_str(&map, "attendee_name")?;
    let attendee_email = require_str(&map, "attendee_email")?;
    let note = take_str(&map, "note");
    let status = take_str(&map, "status")
        .as_deref()
        .map_or(BookingStatus::Confirmed, parse_status);
    let created_utc = take_str(&map, "created_utc").unwrap_or_default();

    Ok(Booking {
        id: BookingId(id),
        path: path.to_string(),
        event_type_id: EventTypeId(event_type_id),
        start_utc,
        end_utc,
        attendee_name,
        attendee_email,
        note,
        status,
        created_utc,
    })
}

fn parse_status(value: &str) -> BookingStatus {
    match value.to_ascii_lowercase().as_str() {
        "pending" => BookingStatus::Pending,
        "confirmed" => BookingStatus::Confirmed,
        "cancelled" | "canceled" => BookingStatus::Cancelled,
        "no_show" | "noshow" | "no-show" => BookingStatus::NoShow,
        "completed" => BookingStatus::Completed,
        _ => BookingStatus::Confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_booking() {
        let yaml = r"
type: scheduling-booking
id: 11111111-aaaa-bbbb-cccc-222222222222
event_type_id: consult-30
start_utc: 2026-06-01T15:00:00Z
end_utc: 2026-06-01T15:30:00Z
attendee_name: Alice
attendee_email: alice@example.com
status: confirmed
created_utc: 2026-05-22T10:00:00Z
note: First call
";
        let b = parse_booking("bookings/xyz.md", yaml).unwrap();
        assert_eq!(b.attendee_name, "Alice");
        assert!(matches!(b.status, BookingStatus::Confirmed));
        assert_eq!(b.note.as_deref(), Some("First call"));
    }
}
