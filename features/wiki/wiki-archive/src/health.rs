//! Extractor health ledger — `task wiki archive health`.
//!
//! Social extractors (phase 3) are explicitly
//! accept-fragility: every shape they scrape drifts. The
//! deal we make instead of pretending stability is a HONEST
//! surface: every archive attempt records success/failure per
//! route into a small JSON ledger, and the `health` verb says
//! which routes currently work, which are broken, and what
//! the last error looked like — so "Reddit is blocked again"
//! is a glance, not an investigation.
//!
//! The ledger is per-org local state (the CLI passes the
//! path); recording never fails the archive itself.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::router::Route;

/// Per-route counters + last outcomes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteHealth {
    pub successes: u64,
    pub failures: u64,
    pub last_success: Option<DateTime<Utc>>,
    pub last_failure: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Route → health, ordered for stable rendering.
pub type HealthLedger = BTreeMap<String, RouteHealth>;

/// Stable label for a route — the ledger key and what the
/// health table prints.
#[must_use]
pub fn route_label(route: &Route) -> &'static str {
    match route {
        Route::Article => "article",
        Route::GoogleDoc { .. } => "google-doc",
        Route::YouTube { .. } => "youtube",
        Route::Video => "video",
        Route::Pdf => "pdf",
        Route::ApplePodcast { .. } => "podcast-apple",
        Route::SpotifyPodcast { .. } => "podcast-spotify",
        Route::Reddit { .. } => "reddit",
        Route::Tweet { .. } => "x",
    }
}

/// Load the ledger (missing/corrupt file = empty — health is
/// advisory, never a blocker).
#[must_use]
pub fn load(path: &Path) -> HealthLedger {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Record one attempt outcome. Errors writing the ledger are
/// swallowed (logged) — an archive must never fail because
/// bookkeeping did.
pub fn record(path: &Path, label: &str, outcome: Result<(), &str>) {
    let mut ledger = load(path);
    let entry = ledger.entry(label.to_string()).or_default();
    let now = Utc::now();
    match outcome {
        Ok(()) => {
            entry.successes += 1;
            entry.last_success = Some(now);
        }
        Err(message) => {
            entry.failures += 1;
            entry.last_failure = Some(now);
            entry.last_error = Some(message.chars().take(300).collect());
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&ledger) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!(error = %e, path = %path.display(), "health ledger write failed");
            }
        }
        Err(e) => tracing::warn!(error = %e, "health ledger serialize failed"),
    }
}

/// Current status of one route, derived from recency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteStatus {
    /// Most recent attempt succeeded.
    Ok,
    /// Most recent attempt failed.
    Broken,
    /// Never attempted.
    Unknown,
}

#[must_use]
pub fn status_of(h: &RouteHealth) -> RouteStatus {
    match (h.last_success, h.last_failure) {
        (None, None) => RouteStatus::Unknown,
        (Some(_), None) => RouteStatus::Ok,
        (None, Some(_)) => RouteStatus::Broken,
        (Some(s), Some(f)) => {
            if s >= f {
                RouteStatus::Ok
            } else {
                RouteStatus::Broken
            }
        }
    }
}

/// Render the health table. Pure — fixture-tested.
#[must_use]
pub fn render(ledger: &HealthLedger) -> String {
    if ledger.is_empty() {
        return "no archive attempts recorded yet — run `task wiki archive <url>` first".into();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{:<16} {:<8} {:>4} {:>4}  {:<20} {}\n",
        "route", "status", "ok", "fail", "last success", "last error"
    ));
    for (label, h) in ledger {
        let status = match status_of(h) {
            RouteStatus::Ok => "ok",
            RouteStatus::Broken => "BROKEN",
            RouteStatus::Unknown => "unknown",
        };
        let last_ok = h.last_success.map_or_else(
            || "never".to_string(),
            |t| t.format("%Y-%m-%d %H:%M").to_string(),
        );
        let last_err = match status_of(h) {
            RouteStatus::Broken => h.last_error.clone().unwrap_or_default(),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{label:<16} {status:<8} {:>4} {:>4}  {last_ok:<20} {last_err}\n",
            h.successes, h.failures
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_status_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wiki-archive-health-{}", std::process::id()));
        let path = dir.join("health.json");
        let _ = std::fs::remove_file(&path);

        record(&path, "reddit", Err("HTTP 403 — blocked"));
        record(&path, "reddit", Ok(()));
        record(&path, "x", Err("every ladder rung failed"));

        let ledger = load(&path);
        let reddit = &ledger["reddit"];
        assert_eq!(reddit.successes, 1);
        assert_eq!(reddit.failures, 1);
        // Success is more recent than failure ⇒ ok.
        assert_eq!(status_of(reddit), RouteStatus::Ok);
        assert_eq!(status_of(&ledger["x"]), RouteStatus::Broken);

        let table = render(&ledger);
        assert!(table.contains("reddit"), "{table}");
        assert!(table.contains("BROKEN"), "{table}");
        assert!(table.contains("every ladder rung failed"), "{table}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let ledger = load(Path::new("/definitely/not/here.json"));
        assert!(ledger.is_empty());
        assert!(render(&ledger).contains("no archive attempts"));
    }
}
