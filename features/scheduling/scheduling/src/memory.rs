//! In-memory backend impls. Each capability sub-trait gets its
//! own impl block so unsupported capabilities (e.g. a future
//! read-only peer) drop trivially. Useful for the demo route +
//! tests; the real `VaultScheduler` lands in a follow-up commit.

use std::sync::Mutex;

use indexmap::IndexMap;

use scheduling_proto::{
    AvailabilitySchedule, Booking, BookingId, BookingStatus, Bookings, DayTemplate, DayTemplateId,
    DayTemplates, EventType, EventTypeId, EventTypes, NewBooking, ScheduleId, Schedules,
    SchedulingError, SlotQuery, Slots, TimeSlot,
};

#[derive(Default)]
pub struct InMemoryScheduler {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    day_templates: IndexMap<String, DayTemplate>,
    event_types: IndexMap<String, EventType>,
    schedules: IndexMap<String, AvailabilitySchedule>,
    bookings: IndexMap<String, Booking>,
}

impl InMemoryScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Test helper: seed an arbitrary entity by id.
    pub fn seed_day_template(&self, dt: DayTemplate) {
        let mut inner = self.inner.lock().expect("scheduler lock");
        inner.day_templates.insert(dt.id.0.clone(), dt);
    }
}

// ── Personal: day templates ──────────────────────────────────────
impl DayTemplates for InMemoryScheduler {
    fn list_day_templates(&self) -> Result<Vec<DayTemplate>, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        Ok(inner.day_templates.values().cloned().collect())
    }

    fn get_day_template(&self, id: &DayTemplateId) -> Result<DayTemplate, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        inner
            .day_templates
            .get(&id.0)
            .cloned()
            .ok_or_else(|| SchedulingError::NotFound { id: id.0.clone() })
    }

    fn upsert_day_template(&self, template: &DayTemplate) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner
            .day_templates
            .insert(template.id.0.clone(), template.clone());
        Ok(())
    }

    fn delete_day_template(&self, id: &DayTemplateId) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner.day_templates.shift_remove(&id.0);
        Ok(())
    }
}

// ── Cal.com-style: event types ───────────────────────────────────
impl EventTypes for InMemoryScheduler {
    fn list_event_types(&self) -> Result<Vec<EventType>, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        Ok(inner.event_types.values().cloned().collect())
    }

    fn get_event_type(&self, id: &EventTypeId) -> Result<EventType, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        inner
            .event_types
            .get(&id.0)
            .cloned()
            .ok_or_else(|| SchedulingError::NotFound { id: id.0.clone() })
    }

    fn upsert_event_type(&self, event_type: &EventType) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner
            .event_types
            .insert(event_type.id.0.clone(), event_type.clone());
        Ok(())
    }

    fn delete_event_type(&self, id: &EventTypeId) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner.event_types.shift_remove(&id.0);
        Ok(())
    }
}

// ── Availability schedules ───────────────────────────────────────
impl Schedules for InMemoryScheduler {
    fn list_schedules(&self) -> Result<Vec<AvailabilitySchedule>, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        Ok(inner.schedules.values().cloned().collect())
    }

    fn get_schedule(&self, id: &ScheduleId) -> Result<AvailabilitySchedule, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        inner
            .schedules
            .get(&id.0)
            .cloned()
            .ok_or_else(|| SchedulingError::NotFound { id: id.0.clone() })
    }

    fn upsert_schedule(&self, schedule: &AvailabilitySchedule) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner
            .schedules
            .insert(schedule.id.0.clone(), schedule.clone());
        Ok(())
    }

    fn delete_schedule(&self, id: &ScheduleId) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner.schedules.shift_remove(&id.0);
        Ok(())
    }
}

// ── Open-slot listing ────────────────────────────────────────────
impl Slots for InMemoryScheduler {
    fn list_open_slots(&self, _query: &SlotQuery) -> Result<Vec<TimeSlot>, SchedulingError> {
        // v1 stub: slot generation lands with the vault-backed
        // impl. The in-memory backend just returns an empty list
        // so the public booking page renders the "no slots"
        // state without crashing.
        Ok(Vec::new())
    }
}

// ── Bookings ─────────────────────────────────────────────────────
impl Bookings for InMemoryScheduler {
    fn list_bookings(&self) -> Result<Vec<Booking>, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        Ok(inner.bookings.values().cloned().collect())
    }

    fn get_booking(&self, id: &BookingId) -> Result<Booking, SchedulingError> {
        let inner = self.inner.lock().map_err(poisoned)?;
        inner
            .bookings
            .get(&id.0)
            .cloned()
            .ok_or_else(|| SchedulingError::NotFound { id: id.0.clone() })
    }

    fn create_booking(&self, booking: &NewBooking) -> Result<Booking, SchedulingError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let persisted = Booking {
            id: BookingId(id.clone()),
            path: format!("Records/bookings/{id}.md"),
            event_type_id: booking.event_type_id.clone(),
            start_utc: booking.start_utc.clone(),
            end_utc: booking.end_utc.clone(),
            attendee_name: booking.attendee_name.clone(),
            attendee_email: booking.attendee_email.clone(),
            note: booking.note.clone(),
            status: BookingStatus::Confirmed,
            created_utc: now,
        };
        let mut inner = self.inner.lock().map_err(poisoned)?;
        inner.bookings.insert(id, persisted.clone());
        Ok(persisted)
    }

    fn update_booking_status(
        &self,
        id: &BookingId,
        status: BookingStatus,
    ) -> Result<(), SchedulingError> {
        let mut inner = self.inner.lock().map_err(poisoned)?;
        if let Some(b) = inner.bookings.get_mut(&id.0) {
            b.status = status;
            Ok(())
        } else {
            Err(SchedulingError::NotFound { id: id.0.clone() })
        }
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> SchedulingError {
    SchedulingError::Backend {
        message: "scheduler mutex poisoned".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scheduling_proto::{BlockCategory, TimeBlock, TimeBlockId, TimeOfDay};

    fn weekday_template() -> DayTemplate {
        DayTemplate {
            path: "Projects/Scheduling/templates/weekday.md".into(),
            id: DayTemplateId("weekday".into()),
            name: "Weekday".into(),
            description: None,
            blocks: vec![TimeBlock {
                id: TimeBlockId("morning".into()),
                start: TimeOfDay::new(6, 0),
                end: TimeOfDay::new(6, 30),
                label: "Morning Reset".into(),
                category: BlockCategory::Reset,
                note: None,
            }]
            .into(),
        }
    }

    #[test]
    fn upsert_then_get_roundtrips() {
        let s = InMemoryScheduler::new();
        let dt = weekday_template();
        s.upsert_day_template(&dt).unwrap();
        let got = s.get_day_template(&dt.id).unwrap();
        assert_eq!(got.name, "Weekday");
    }

    #[test]
    fn delete_removes() {
        let s = InMemoryScheduler::new();
        let dt = weekday_template();
        s.upsert_day_template(&dt).unwrap();
        s.delete_day_template(&dt.id).unwrap();
        assert!(matches!(
            s.get_day_template(&dt.id),
            Err(SchedulingError::NotFound { .. })
        ));
    }

    /// Compile-time check: a function bound to two capabilities
    /// picks them up from `InMemoryScheduler` without an umbrella
    /// trait. The body is intentionally trivial — what matters is
    /// the bound resolving.
    fn bound_compiles<S: DayTemplates + Bookings>(_: &S) {}

    #[test]
    fn sub_trait_bounds_resolve() {
        let s = InMemoryScheduler::new();
        bound_compiles(&s);
    }
}
