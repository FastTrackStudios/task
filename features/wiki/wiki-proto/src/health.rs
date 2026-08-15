//! Quick wiki-state snapshot. Useful for CLI dashboards
//! (`task wiki status`), UI badges, and oncall — answers
//! "what's the wiki up to right now?" in one round trip.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct WikiHealth {
    /// `Wiki/` exists and bootstrap has been run.
    pub bootstrap_done: bool,
    /// `Wiki/schema.md` present + non-empty.
    pub schema_present: bool,
    /// `Wiki/purpose.md` present + non-empty.
    pub purpose_present: bool,
    /// Pages tracked by the index.
    pub page_count: u32,
    /// Files under `Wiki/raw/sources/`.
    pub source_count: u32,
    /// `Pending` + `Analyzing` + `Generating` + `Writing`.
    pub queue_depth: u32,
    /// `Failed` tasks awaiting retry or curator decision.
    pub queue_failed: u32,
    /// Lint findings in `Open` state.
    pub open_findings: u32,
    /// Review items in `Open` state.
    pub open_reviews: u32,
    /// Research plans in `Proposed` / `Running` / `Awaiting`.
    pub research_in_flight: u32,
    /// Registered federation peers.
    pub peer_count: u32,
    /// Last successful lint pass, if any.
    pub last_lint_at: Option<DateTime<Utc>>,
    /// Last `IngestStatus::Done` transition, if any.
    pub last_ingest_at: Option<DateTime<Utc>>,
    /// Whether the backend's source watcher is currently
    /// active (mirrors [`crate::service::WikiService::is_watching`]).
    pub watching: bool,
}
