//! Operator CPU profiling — the server profiles itself.
//!
//! Production runs where nobody can `perf` (no root on the node), so
//! when the process burns cores with nothing in the logs the only tool
//! that can see inside it is the process. Two routes on the server-
//! management surface, gated by [`crate::operator::is_operator`]:
//!
//! - `GET /server/debug/profile?seconds=N&format=flamegraph|pprof` —
//!   sample every thread at 99 Hz for `N` seconds (default 10, max 60)
//!   and answer with an SVG flamegraph or a gzipped pprof protobuf.
//! - `GET /server/debug/threads` — `/proc/self/task/*/stat` read twice a
//!   second apart: per-thread CPU%, named from `comm`, hottest first.
//!   Pure `/proc`; the thing to run first, because it says *which*
//!   threads are hot before a flamegraph says why.
//!
//! The sampler is signal-based (`pprof`), which only exists on Linux;
//! anywhere else both routes answer 501. Only one profile can run at a
//! time — a second caller during a window gets 409.
//!
//! Wide-event fields: `debug.profile_seconds`, `debug.format`, and
//! `auth.principal_kind` (set by the operator gate). No log lines.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::AppState;

/// `?seconds=` when absent.
pub const DEFAULT_SECONDS: u64 = 10;
/// `?seconds=` ceiling — a profile holds a SIGPROF timer on every thread
/// for its whole window, so it is bounded.
pub const MAX_SECONDS: u64 = 60;

/// The 401 body; one line, no hint about which check failed.
const REFUSED: &str =
    "operator role required: the server's static MCP token, or `admin` in the home org\n";

#[derive(Debug, Deserialize, Default)]
pub struct ProfileQuery {
    pub seconds: Option<u64>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Flamegraph,
    Pprof,
}

impl Format {
    fn parse(s: Option<&str>) -> Option<Self> {
        match s {
            None | Some("flamegraph") | Some("svg") => Some(Self::Flamegraph),
            Some("pprof") => Some(Self::Pprof),
            Some(_) => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Flamegraph => "flamegraph",
            Self::Pprof => "pprof",
        }
    }
}

/// Clamp `?seconds=` into `1..=MAX_SECONDS`, defaulting when absent.
pub fn clamp_seconds(requested: Option<u64>) -> u64 {
    requested.unwrap_or(DEFAULT_SECONDS).clamp(1, MAX_SECONDS)
}

fn refused() -> Response {
    (StatusCode::UNAUTHORIZED, REFUSED).into_response()
}

/// `GET /server/debug/profile`.
pub async fn profile_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ProfileQuery>,
) -> Response {
    if !crate::operator::is_operator(&state, &headers).await {
        return refused();
    }
    let seconds = clamp_seconds(q.seconds);
    let Some(format) = Format::parse(q.format.as_deref()) else {
        return (
            StatusCode::BAD_REQUEST,
            "format must be `flamegraph` or `pprof`\n",
        )
            .into_response();
    };
    architect_telemetry::wide::set("debug.profile_seconds", seconds as i64);
    architect_telemetry::wide::set("debug.format", format.as_str());
    sample(seconds, format).await
}

/// `GET /server/debug/threads`.
pub async fn threads_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::operator::is_operator(&state, &headers).await {
        return refused();
    }
    threads().await
}

#[cfg(target_os = "linux")]
async fn sample(seconds: u64, format: Format) -> Response {
    use std::io::Write as _;

    let guard = match pprof::ProfilerGuardBuilder::default()
        .frequency(99)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
    {
        Ok(g) => g,
        // pprof keeps one global profiler; a second `build` while a
        // window is open is the only way this fails in practice.
        Err(e) => {
            return (
                StatusCode::CONFLICT,
                format!("a profile is already running: {e}\n"),
            )
                .into_response();
        }
    };
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

    // Symbolising a report walks every sampled frame through the
    // symbol table — blocking work, off the runtime.
    let rendered = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let report = guard.report().build().map_err(|e| e.to_string())?;
        drop(guard);
        match format {
            Format::Flamegraph => {
                let mut svg = Vec::new();
                report.flamegraph(&mut svg).map_err(|e| e.to_string())?;
                // SIGPROF only fires while the process is on a CPU, so
                // an idle window has no frames and pprof writes nothing.
                // Answer with an SVG that says so rather than 0 bytes.
                if svg.is_empty() {
                    svg = idle_svg(seconds).into_bytes();
                }
                Ok(svg)
            }
            Format::Pprof => {
                use pprof::protos::Message as _;
                let profile = report.pprof().map_err(|e| e.to_string())?;
                let raw = profile.encode_to_vec();
                let mut gz =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                gz.write_all(&raw).map_err(|e| e.to_string())?;
                gz.finish().map_err(|e| e.to_string())
            }
        }
    })
    .await;

    match rendered {
        Ok(Ok(body)) => {
            let (content_type, filename) = match format {
                Format::Flamegraph => ("image/svg+xml", "profile.svg"),
                Format::Pprof => ("application/octet-stream", "profile.pb.gz"),
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type.to_owned()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                body,
            )
                .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("profile failed: {e}\n"),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("profile worker panicked: {e}\n"),
        )
            .into_response(),
    }
}

/// The flamegraph for a window in which no thread ran.
#[cfg(target_os = "linux")]
fn idle_svg(seconds: u64) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"640\" height=\"48\">\
         <text x=\"8\" y=\"30\" font-family=\"monospace\" font-size=\"14\">\
         no CPU samples in {seconds}s: the process was idle for the whole window\
         </text></svg>\n"
    )
}

#[cfg(not(target_os = "linux"))]
async fn sample(_seconds: u64, _format: Format) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "CPU profiling is Linux-only on this build\n",
    )
        .into_response()
}

/// One row of `GET /server/debug/threads`.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ThreadCpu {
    pub tid: u32,
    pub name: String,
    pub cpu_pct: f64,
}

#[cfg(target_os = "linux")]
async fn threads() -> Response {
    let before = proc_threads::snapshot();
    let started = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let after = proc_threads::snapshot();
    let rows = proc_threads::diff(&before, &after, started.elapsed());
    axum::Json(rows).into_response()
}

#[cfg(not(target_os = "linux"))]
async fn threads() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "per-thread CPU needs /proc; Linux-only on this build\n",
    )
        .into_response()
}

/// `/proc/self/task/<tid>/{stat,comm}` → per-thread CPU. Kept
/// dependency-free on purpose: when the process is the problem, the
/// diagnostic should lean on nothing that shares its fate.
#[cfg(target_os = "linux")]
pub mod proc_threads {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::ThreadCpu;

    /// `/proc` reports CPU time in `USER_HZ` ticks, which the kernel
    /// fixes at 100 regardless of the scheduler's `HZ` — the value
    /// `sysconf(_SC_CLK_TCK)` returns on every Linux since 2.6.
    pub const USER_HZ: f64 = 100.0;

    /// `tid → (comm, utime+stime ticks)`.
    pub type Snapshot = BTreeMap<u32, (String, u64)>;

    pub fn snapshot() -> Snapshot {
        let mut out = Snapshot::new();
        let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
            return out;
        };
        for entry in dir.flatten() {
            let Some(tid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let base = entry.path();
            let Ok(stat) = std::fs::read_to_string(base.join("stat")) else {
                continue;
            };
            let Some(ticks) = cpu_ticks(&stat) else {
                continue;
            };
            let name = std::fs::read_to_string(base.join("comm"))
                .map(|s| s.trim().to_owned())
                .unwrap_or_default();
            out.insert(tid, (name, ticks));
        }
        out
    }

    /// `utime + stime` from one `/proc/<pid>/task/<tid>/stat` line. The
    /// comm field is parenthesised and may itself contain spaces or
    /// parentheses, so split after the *last* `)`; from there `utime` and
    /// `stime` are the 12th and 13th whitespace-separated fields
    /// (fields 14 and 15 of the documented 1-based layout).
    pub fn cpu_ticks(stat: &str) -> Option<u64> {
        let rest = &stat[stat.rfind(')')? + 1..];
        let mut fields = rest.split_whitespace();
        let utime: u64 = fields.nth(11)?.parse().ok()?;
        let stime: u64 = fields.next()?.parse().ok()?;
        Some(utime + stime)
    }

    /// Rows for every thread present in both snapshots, hottest first.
    pub fn diff(before: &Snapshot, after: &Snapshot, elapsed: Duration) -> Vec<ThreadCpu> {
        let secs = elapsed.as_secs_f64().max(f64::EPSILON);
        let mut rows: Vec<ThreadCpu> = after
            .iter()
            .filter_map(|(tid, (name, ticks_after))| {
                let (_, ticks_before) = before.get(tid)?;
                let delta = ticks_after.saturating_sub(*ticks_before) as f64;
                let cpu_pct = (delta / USER_HZ) / secs * 100.0;
                Some(ThreadCpu {
                    tid: *tid,
                    name: name.clone(),
                    cpu_pct: (cpu_pct * 10.0).round() / 10.0,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.tid.cmp(&b.tid))
        });
        rows
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cpu_ticks_reads_past_a_hostile_comm() {
            // A comm with spaces and a `)` in it, then the documented
            // field layout: state ppid pgrp session tty tpgid flags
            // minflt cminflt majflt cmajflt utime stime …
            let line = "42 (tokio (rt) w) R 1 1 1 0 -1 4194560 10 0 0 0 250 75 0 0 20 0 9 0 5 0 0";
            assert_eq!(cpu_ticks(line), Some(325));
            assert_eq!(cpu_ticks("garbage"), None);
        }

        #[test]
        fn diff_sorts_hottest_first_and_drops_new_threads() {
            let mut before = Snapshot::new();
            before.insert(1, ("main".into(), 100));
            before.insert(2, ("tokio-runtime-w".into(), 100));
            let mut after = Snapshot::new();
            after.insert(1, ("main".into(), 110));
            after.insert(2, ("tokio-runtime-w".into(), 190));
            after.insert(3, ("newborn".into(), 5));
            let rows = diff(&before, &after, Duration::from_secs(1));
            assert_eq!(rows.len(), 2, "a thread with no baseline has no rate");
            assert_eq!(rows[0].tid, 2);
            assert_eq!(rows[0].cpu_pct, 90.0);
            assert_eq!(rows[1].cpu_pct, 10.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_default_and_clamp() {
        assert_eq!(clamp_seconds(None), DEFAULT_SECONDS);
        assert_eq!(clamp_seconds(Some(0)), 1);
        assert_eq!(clamp_seconds(Some(600)), MAX_SECONDS);
        assert_eq!(clamp_seconds(Some(15)), 15);
    }

    #[test]
    fn format_parses_the_two_encodings() {
        assert_eq!(Format::parse(None), Some(Format::Flamegraph));
        assert_eq!(Format::parse(Some("pprof")), Some(Format::Pprof));
        assert_eq!(Format::parse(Some("perf")), None);
    }
}
