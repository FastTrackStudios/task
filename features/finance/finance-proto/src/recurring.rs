//! Recurring schedules — drive generation of invoices, quotes,
//! or expenses on a cadence.
//!
//! The schedule points at a template row whose
//! `kind = InvoiceKind::RecurringTemplate`. The generator
//! clones the template into a real invoice, bumps
//! `next_run_at`, and decrements `remaining_cycles`.

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

/// Cadence options. Lifted from InvoiceNinja's 12-value enum.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Facet,
    serde::Serialize,
    serde::Deserialize,
)]
#[repr(u8)]
pub enum RecurringFrequency {
    Daily,
    Weekly,
    Biweekly,
    FourWeeks,
    Monthly,
    TwoMonths,
    Quarterly,
    FourMonths,
    SixMonths,
    Annually,
    TwoYears,
    ThreeYears,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Facet,
    serde::Serialize,
    serde::Deserialize,
)]
#[repr(u8)]
pub enum RecurringStatus {
    Draft,
    Active,
    Paused,
    Completed,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_recurring_schedules", repo)]
pub struct RecurringSchedule {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    /// The template invoice row that gets cloned each cycle.
    #[architect(filterable)]
    pub template_invoice_id: Uuid,

    #[architect(json, filterable)]
    pub frequency: RecurringFrequency,

    #[architect(json, filterable)]
    pub status: RecurringStatus,

    /// ISO-8601 date — first run.
    pub start_date: String,

    /// Next time the generator should fire.
    #[architect(sortable)]
    pub next_run_at: DateTime<Utc>,

    /// `0` = unlimited (matches InvoiceNinja's nullable cycles).
    pub remaining_cycles: i32,

    /// Sentinel: epoch zero = never run.
    pub last_run_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}
