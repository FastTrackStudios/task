//! `Booking` → markdown frontmatter.

use scheduling_proto::{Booking, BookingStatus};

use super::{WriteError, yaml_to_page};

pub fn serialize_booking(b: &Booking) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-booking".into());
    map.insert("id".into(), b.id.0.clone().into());
    map.insert("event_type_id".into(), b.event_type_id.0.clone().into());
    map.insert("start_utc".into(), b.start_utc.clone().into());
    map.insert("end_utc".into(), b.end_utc.clone().into());
    map.insert("attendee_name".into(), b.attendee_name.clone().into());
    map.insert("attendee_email".into(), b.attendee_email.clone().into());
    map.insert("status".into(), status_label(b.status).into());
    map.insert("created_utc".into(), b.created_utc.clone().into());
    if let Some(n) = &b.note {
        map.insert("note".into(), n.clone().into());
    }
    yaml_to_page(map)
}

fn status_label(s: BookingStatus) -> &'static str {
    match s {
        BookingStatus::Pending => "pending",
        BookingStatus::Confirmed => "confirmed",
        BookingStatus::Cancelled => "cancelled",
        BookingStatus::NoShow => "no_show",
        BookingStatus::Completed => "completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_booking;
    use scheduling_proto::{BookingId, EventTypeId};

    #[test]
    fn round_trip_booking() {
        let b = Booking {
            id: BookingId("xyz".into()),
            path: "bookings/xyz.md".into(),
            event_type_id: EventTypeId("consult-30".into()),
            start_utc: "2026-06-01T15:00:00Z".into(),
            end_utc: "2026-06-01T15:30:00Z".into(),
            attendee_name: "Alice".into(),
            attendee_email: "alice@example.com".into(),
            note: Some("First call".into()),
            status: BookingStatus::Confirmed,
            created_utc: "2026-05-22T10:00:00Z".into(),
        };
        let md = serialize_booking(&b).unwrap();
        let inner = md.trim_start_matches("---\n").trim_end_matches("---\n");
        let parsed = parse_booking("bookings/xyz.md", inner).unwrap();
        assert_eq!(parsed.attendee_name, "Alice");
        assert!(matches!(parsed.status, BookingStatus::Confirmed));
    }
}
