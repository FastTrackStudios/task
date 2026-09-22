//! Chapter — a gigabyte take, read from the server that holds it.
//!
//! `scale.rs` and `federation_stream.rs` prove the *mechanics* of large
//! media — chunked storage, ranged reads, relaying a chunk at a time —
//! at a few megabytes, because that is what a suite that runs on every
//! change can afford. This chapter is the same path at the size Session
//! and Signal actually produce: a multi-gigabyte recording, offered by
//! ACME, accepted by VNT, and read by VNT through the relay.
//!
//! It is a measurement as much as a test, so it is opt-in:
//!
//! ```bash
//! TASK_SCALE_BYTES=$((2<<30)) cargo nextest run -p integration \
//!     --run-ignored only -E 'test(/^large_media::/)' --no-capture
//! ```
//!
//! What it asserts is what must hold at any size: every byte arrives
//! intact (hashed as it streams, never held), and memory does not grow
//! with the file — the relay costs one chunk, not one file. What it
//! prints is what the numbers are: ingest, first byte, throughput, a
//! seek, and whether a second read is any cheaper than the first (i.e.
//! whether the receiving server keeps what it relayed).

use std::io::Write as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use files::path::RootPath;
use files::service::access::Capability;
use files::service::federation::EndpointId;
use integration::scenario::Scenario;

const DEFAULT_BYTES: u64 = 1 << 30;
const MIB: f64 = (1u64 << 20) as f64;

fn scale_bytes() -> u64 {
    std::env::var("TASK_SCALE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_BYTES)
}

/// Incompressible and reproducible: a recording does not dedup against
/// itself, and a file of zeros would make chunking look free.
fn write_take(path: &std::path::Path, len: u64) -> blake3::Hash {
    let mut file =
        std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path).expect("the take"));
    let mut hasher = blake3::Hasher::new();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut buf = vec![0u8; 1 << 20];
    let mut left = len;
    while left > 0 {
        // `chunks_mut`, not `chunks_exact_mut`: the buffer is a multiple
        // of eight, so both walk the same words, and newer clippy
        // (1.98, CI's) rejects the exact form with a constant size.
        for word in buf.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            word.copy_from_slice(&state.to_le_bytes());
        }
        let n = left.min(buf.len() as u64) as usize;
        file.write_all(&buf[..n]).expect("write the take");
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    file.flush().expect("flush the take");
    hasher.finalize()
}

/// Counts and hashes what passes through it, and notes when the first
/// byte arrived — the one number a player's user actually feels.
struct Measure {
    started: Instant,
    first_byte: Option<Duration>,
    bytes: u64,
    hasher: blake3::Hasher,
}

impl Measure {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            first_byte: None,
            bytes: 0,
            hasher: blake3::Hasher::new(),
        }
    }
}

impl tokio::io::AsyncWrite for Measure {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.first_byte.is_none() && !buf.is_empty() {
            self.first_byte = Some(self.started.elapsed());
        }
        self.bytes += buf.len() as u64;
        self.hasher.update(buf);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// `(VmRSS, VmHWM)` in bytes — both servers live in this process, so
/// this is the whole system's footprint.
fn memory() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map_or(0, |kib| kib * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Resets the high-water mark, so each phase's peak is its own.
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

struct Row {
    phase: &'static str,
    bytes: u64,
    elapsed: Duration,
    first_byte: Option<Duration>,
    peak_growth: u64,
}

async fn timed<F, Fut>(phase: &'static str, rows: &mut Vec<Row>, f: F) -> Measure
where
    F: FnOnce(Measure) -> Fut,
    Fut: std::future::Future<Output = Measure>,
{
    reset_peak();
    let (before, _) = memory();
    let started = Instant::now();
    let m = f(Measure::new()).await;
    let elapsed = started.elapsed();
    eprintln!("[large_media] {phase}: {:.1}s", elapsed.as_secs_f64());
    let (_, peak) = memory();
    rows.push(Row {
        phase,
        bytes: m.bytes,
        elapsed,
        first_byte: m.first_byte,
        peak_growth: peak.saturating_sub(before),
    });
    m
}

/// t[verify files.scale.large-media]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement at gigabyte scale — run it on purpose"]
async fn a_gigabyte_take_crosses_to_another_server_intact_and_in_bounded_memory() {
    let len = scale_bytes();
    let s = Scenario::open().await;
    let mut rows = Vec::new();

    // ── ACME records the take ─────────────────────────────────────────
    let tree = s.orgs.acme.tree().join("SessionLarge");
    std::fs::create_dir_all(tree.join("Audio Files")).expect("the session folder");
    let started = Instant::now();
    let written = write_take(&tree.join("Audio Files").join("big-take.wav"), len);
    rows.push(Row {
        phase: "write to disk",
        bytes: len,
        elapsed: started.elapsed(),
        first_byte: None,
        peak_growth: 0,
    });

    reset_peak();
    let (before, _) = memory();
    let started = Instant::now();
    eprintln!("[large_media] written; adopting");
    let root = integration::orgs::adopt(&s.orgs.acme, "SessionLarge").await;
    eprintln!(
        "[large_media] adopted: {:.1}s",
        started.elapsed().as_secs_f64()
    );
    rows.push(Row {
        phase: "adopt (hash + chunk)",
        bytes: len,
        elapsed: started.elapsed(),
        first_byte: None,
        peak_growth: memory().1.saturating_sub(before),
    });
    files::service::access::AccessService::grant(
        &s.orgs.acme.backend,
        s.people.alice.subject.clone(),
        root,
        RootPath::root(),
        task_server::example_org::Holds::Owner.capabilities(),
    )
    .await
    .expect("ACME grants Alice the session");

    // ── the origin reading its own file: the baseline ─────────────────
    let alice = s.as_alice().await;
    let local = alice
        .media()
        .await
        .read(
            root,
            RootPath::parse("Audio Files/big-take.wav").expect("path"),
        )
        .await
        .expect("a ticket at the origin");
    let acme = s.orgs.acme.backend.clone();
    let origin = timed("origin, whole file", &mut rows, |mut m| async {
        acme.redeem_bytes(&local.token, None, &mut m)
            .await
            .expect("the origin streams its own take");
        m
    })
    .await;
    assert_eq!(
        origin.hasher.finalize(),
        written,
        "the origin read back different bytes"
    );

    // ── offered, accepted, resolved ───────────────────────────────────
    let offer = alice
        .federation()
        .await
        .offer(
            root,
            RootPath::parse("Audio Files").expect("the offered subtree"),
            EndpointId(s.orgs.vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("ACME offers the audio");
    let victor = s.as_victor().await;
    victor
        .federation()
        .await
        .accept(offer)
        .await
        .expect("VNT accepts");
    let resolved = victor
        .federation()
        .await
        .resolve_content(
            root,
            RootPath::parse("Audio Files/big-take.wav").expect("path"),
        )
        .await
        .expect("resolve")
        .expect("VNT accepted an offer containing the take");
    let ticket = victor
        .media()
        .await
        .read(resolved.root_id, resolved.path)
        .await
        .expect("a ticket on the accepted root");

    // ── VNT reads it, twice, then seeks ───────────────────────────────
    let vnt = s.orgs.vnt.backend.clone();
    for phase in ["relayed, first read", "relayed, second read"] {
        let m = timed(phase, &mut rows, |mut m| async {
            vnt.redeem_bytes(&ticket.token, None, &mut m)
                .await
                .expect("VNT streams the take through the relay");
            m
        })
        .await;
        assert_eq!(m.bytes, len, "{phase}: short read");
        assert_eq!(
            m.hasher.finalize(),
            written,
            "{phase}: the bytes changed in transit"
        );
    }
    // The last 4 MiB — what a player asks for when scrubbed to the end.
    let tail = len.saturating_sub(4 << 20);
    timed("relayed, seek to last 4 MiB", &mut rows, |mut m| async {
        vnt.redeem_bytes(&ticket.token, Some((tail, len - 1)), &mut m)
            .await
            .expect("VNT serves a range near the end");
        m
    })
    .await;

    // ── the table ─────────────────────────────────────────────────────
    println!("\nlarge media, {:.0} MiB", len as f64 / MIB);
    println!(
        "{:<30} {:>10} {:>10} {:>12} {:>12} {:>14}",
        "phase", "MiB", "secs", "MiB/s", "first byte", "peak RSS +MiB"
    );
    for r in &rows {
        let secs = r.elapsed.as_secs_f64();
        println!(
            "{:<30} {:>10.0} {:>10.2} {:>12.1} {:>12} {:>14.1}",
            r.phase,
            r.bytes as f64 / MIB,
            secs,
            r.bytes as f64 / MIB / secs.max(f64::EPSILON),
            r.first_byte
                .map_or("-".into(), |d| format!("{:.1}ms", d.as_secs_f64() * 1e3)),
            r.peak_growth as f64 / MIB,
        );
    }

    // Bounded memory is the claim that must hold at any size: a relay
    // that buffered the file would grow by the file.
    for r in rows.iter().filter(|r| r.phase.starts_with("relayed")) {
        assert!(
            r.peak_growth < (len / 4).max(256 << 20),
            "{}: peak RSS grew {:.0} MiB for a {:.0} MiB file — something is holding it whole",
            r.phase,
            r.peak_growth as f64 / MIB,
            len as f64 / MIB
        );
    }
}
