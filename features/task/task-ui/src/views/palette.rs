//! Static class mappings for status + priority — tailwind sees
//! every variant as a literal at scan time. Same rationale as
//! `view-calendar/components/style.rs`.

use task_proto::{Priority, Status};

/// Pill classes for a [`Status`] badge.
#[must_use]
pub fn status_pill(status: Status) -> &'static str {
    match status {
        Status::Open => "bg-slate-700/60 text-slate-200 border border-slate-600",
        Status::InProgress => "bg-sky-700/50 text-sky-100 border border-sky-500",
        Status::Waiting => "bg-amber-700/40 text-amber-100 border border-amber-500",
        Status::Done => "bg-emerald-700/40 text-emerald-100 border border-emerald-500",
        Status::Cancelled => "bg-rose-800/40 text-rose-200 border border-rose-600 line-through",
    }
}

/// Pill classes for a [`Priority`] badge.
#[must_use]
pub fn priority_pill(p: Priority) -> &'static str {
    match p {
        Priority::None => "bg-transparent text-muted-foreground border border-border",
        Priority::Low => "bg-slate-700/50 text-slate-200 border border-slate-500",
        Priority::Normal => "bg-sky-700/40 text-sky-100 border border-sky-500",
        Priority::High => "bg-amber-700/50 text-amber-50 border border-amber-400",
        Priority::Critical => "bg-rose-700/60 text-rose-50 border border-rose-400 font-semibold",
    }
}

/// Left-edge color stripe for a kanban card — keeps the card
/// itself neutral but tints the rail.
#[must_use]
pub fn priority_rail(p: Priority) -> &'static str {
    match p {
        Priority::None => "bg-slate-600",
        Priority::Low => "bg-slate-400",
        Priority::Normal => "bg-sky-400",
        Priority::High => "bg-amber-400",
        Priority::Critical => "bg-rose-500",
    }
}
