// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `cycle` — cyclic life-calendar primitives.
//!
//! Reshapes the year into 4 quarters × (3 cycles × 4 weeks +
//! 1 reset week) = **52 uniform weeks**, optionally with a
//! 53rd "week zero" bonus week in cyclic-leap years (e.g.
//! 2026 / 2032 / 2037 for Monday-start). Every cycle is
//! exactly 28 days, starts on the same weekday, and has the
//! same number of each weekday — see
//! `plans/cyclic-life-calendar.md` for the design rationale.
//!
//! ## Public surface
//!
//! - [`Cycle`] — `architect::Entity` for one 4-week cycle.
//!   UUID PK, deterministic via UUIDv5 over
//!   `cycle:<year>:Q<n>:C<m>:<start>` so the same cycle on
//!   two machines gets the same id without coordination.
//! - [`Quarter`] — `architect::Entity` for a 13-week
//!   quarter (3 cycles + 1 reset week + optional bonus
//!   week). Same deterministic UUIDv5 strategy.
//! - [`generate_year`] — pure function. Given `start_year`,
//!   `year_start_day` (Mon / Sun / anything), and
//!   `first_week_anchor` ([`FirstWeekRule`]), returns
//!   `Vec<Quarter>` with their cycles populated.
//! - [`cycle_for_date`] / [`current_cycle`] — pick the cycle
//!   a given (or today's) date falls inside.
//!
//! Storage is optional. The generator is the source of
//! truth; `--features server` lets you mirror the output
//! into SQLite if you want indexed lookups.

mod generator;
mod model;

pub use generator::{
    FirstWeekRule, current_cycle, cycle_for_date, generate_year, has_bonus_week, week_one_start,
};
pub use model::{Cycle, Cycles, Quarter, weekday_from_short, weekday_to_short};

/// UUIDv5 namespace used to derive deterministic ids from
/// (year, quarter, cycle) tuples. Keeps the same physical
/// cycle stable across machines without needing to coordinate
/// which random UUIDs got minted where.
pub const CYCLE_NS: uuid::Uuid = uuid::Uuid::from_u128(0x636f_6479_5f63_7963_6c69_635f_6e73_5f00);
