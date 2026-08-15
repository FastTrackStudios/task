//! Booking CRUD. The public booking page calls `create_booking`;
//! the host's inbox calls `list_bookings` + `update_booking_status`.

use crate::booking::{Booking, BookingId, BookingStatus, NewBooking};
use crate::error::SchedulingError;

#[architect::rpc]
pub trait Bookings {
    fn list_bookings(&self) -> Result<Vec<Booking>, SchedulingError>;
    fn get_booking(&self, id: &BookingId) -> Result<Booking, SchedulingError>;
    /// Commit a new booking. Returns the persisted form (with id +
    /// path + status). Fails with `SlotUnavailable` if the slot
    /// was already taken between the open-slots query + this call.
    fn create_booking(&self, booking: &NewBooking) -> Result<Booking, SchedulingError>;
    fn update_booking_status(
        &self,
        id: &BookingId,
        status: BookingStatus,
    ) -> Result<(), SchedulingError>;
}
