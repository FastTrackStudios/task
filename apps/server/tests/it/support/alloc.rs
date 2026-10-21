//! A counting global allocator for this test binary: live heap bytes and
//! their high-water mark, so a test can hold the server to a memory
//! budget in bytes the *program* asked for — the number a regression
//! moves — rather than RSS, which also counts what the allocator kept,
//! the binary's own pages and every other test's leftovers.
//!
//! Installed once for the whole `it` binary (`main.rs`). The cost is two
//! relaxed atomics per allocation, which nothing else in the suite can
//! see; every test still runs in a process of its own under nextest, so
//! a reading is one test's and nobody else's.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// [`System`], counted.
pub struct Counting;

fn grew(by: usize) {
    let now = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

fn shrank(by: usize) {
    LIVE.fetch_sub(by, Ordering::Relaxed);
}

// SAFETY: every method forwards to `System` unchanged; the counters are
// bookkeeping beside it and never touch the memory handed out.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded verbatim.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded verbatim.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded verbatim.
        unsafe { System.dealloc(ptr, layout) };
        shrank(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded verbatim.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                shrank(layout.size() - new_size);
            }
        }
        p
    }
}

/// Whether this test has the process, and so the counters, to itself.
///
/// nextest runs every test in a process of its own and sets `NEXTEST`;
/// plain `cargo test` runs the whole `it` binary's tests on threads of one
/// process, where every other test's allocations land in the same
/// counters and a budget reads their work as this test's. A budget test
/// skips itself there (with a line saying so) rather than fail at random.
#[must_use]
pub fn counters_are_this_tests() -> bool {
    if std::env::var_os("NEXTEST").is_some() {
        return true;
    }
    eprintln!(
        "skipped: the heap counters are process-wide and other tests share this process; \
         run it under `cargo nextest run`"
    );
    false
}

/// Heap bytes live right now.
#[must_use]
pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// The most heap bytes live at once since the last [`reset_peak`] (or
/// since the process started).
#[must_use]
pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Start a new high-water mark from what is live now — so the peak of
/// one call can be read as `peak() - live_before`.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// This process's anonymous resident memory, in KiB (`RssAnon` in
/// `/proc/self/status`) — the part of RSS that is heap, stacks and
/// anonymous maps rather than the binary's pages. `None` off Linux.
#[must_use]
pub fn rss_anon_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("RssAnon:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

/// `bytes` as MiB with one decimal, for a report line.
#[must_use]
pub fn mib(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / f64::from(1u32 << 20))
}
