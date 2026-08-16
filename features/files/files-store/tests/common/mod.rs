//! Shared test harness: a deterministic, position-addressable pseudo-random
//! byte generator exposed as a [`tokio::io::AsyncRead`], so integration
//! tests can drive multi-GB sources through `files_store::chunk`
//! without ever materializing a multi-GB `Vec<u8>` in the test process
//! itself — the point of the "no whole-file buffering" acceptance
//! criterion would be lost if the *test* buffered the file.
//!
//! Not every test binary uses every helper below (each `tests/*.rs` file
//! compiles this module fresh).
#![allow(dead_code)]

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};

const BLOCK_SIZE: usize = 64 * 1024;

/// Env var that gates the multi-GiB stress tests. They're marked
/// `#[ignore]` (so plain `cargo test` / CI never runs them), and each one
/// checks this too as a second, explicit opt-in — `--ignored` alone can be
/// passed by habit or by a broader test-running wrapper, and these tests
/// stream multiple GiB through `tempfile::tempdir()`, which is tmpfs (real
/// RAM, not disk) on a typical Linux box.
///
/// Run them with:
/// `FILES_CHUNK_STORE_STRESS=1 cargo test -p task-files-chunk-store -- --ignored`
pub const STRESS_ENV_VAR: &str = "FILES_CHUNK_STORE_STRESS";

/// Returns `true` if the multi-GiB stress tests should actually run their
/// heavy body. If not, the caller should skip with a clear message rather
/// than silently doing nothing.
pub fn stress_tests_enabled() -> bool {
    std::env::var(STRESS_ENV_VAR).is_ok()
}

/// Fill a `BLOCK_SIZE`-aligned block deterministically from `(seed,
/// block_index)`: one blake3 hash seeds a splitmix64 stream, which is fast
/// enough to generate multiple GiB in a `cargo test` run.
fn fill_block(seed: u64, block_index: u64, out: &mut [u8; BLOCK_SIZE]) {
    let mut input = [0u8; 16];
    input[0..8].copy_from_slice(&seed.to_le_bytes());
    input[8..16].copy_from_slice(&block_index.to_le_bytes());
    let digest = blake3::hash(&input);
    let mut state = u64::from_le_bytes(digest.as_bytes()[0..8].try_into().unwrap()) | 1;
    let mut i = 0;
    while i < out.len() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let bytes = z.to_le_bytes();
        let take = bytes.len().min(out.len() - i);
        out[i..i + take].copy_from_slice(&bytes[..take]);
        i += take;
    }
}

/// Write `len` deterministic pseudo-random bytes for `seed`, starting at
/// absolute position `start`, into `out`. Positional: the bytes at a given
/// absolute offset are the same regardless of how the read is split into
/// calls, which is what lets [`EditedSource`] below share unedited bytes
/// exactly with the base source.
fn fill_range(seed: u64, start: u64, out: &mut [u8]) {
    let mut pos = start;
    let mut written = 0usize;
    let mut block = [0u8; BLOCK_SIZE];
    while written < out.len() {
        let block_index = pos / BLOCK_SIZE as u64;
        fill_block(seed, block_index, &mut block);
        let offset_in_block = (pos % BLOCK_SIZE as u64) as usize;
        let avail = BLOCK_SIZE - offset_in_block;
        let take = avail.min(out.len() - written);
        out[written..written + take]
            .copy_from_slice(&block[offset_in_block..offset_in_block + take]);
        written += take;
        pos += take as u64;
    }
}

/// An [`AsyncRead`] source of `len` deterministic pseudo-random bytes,
/// optionally with a sub-range `[edit_start, edit_start + edit_len)`
/// generated from a different seed — simulating a small in-place edit to
/// an otherwise-unchanged multi-GB file. Every byte outside the edit
/// window is bit-for-bit identical to the same-`base_seed`/`len` source
/// with no edit, at the same absolute offset.
pub struct DeterministicSource {
    len: u64,
    pos: u64,
    base_seed: u64,
    edit: Option<(u64, u64, u64)>, // (start, len, seed)
}

impl DeterministicSource {
    pub fn new(seed: u64, len: u64) -> Self {
        Self {
            len,
            pos: 0,
            base_seed: seed,
            edit: None,
        }
    }

    /// Same content as `new(seed, len)`, except bytes in
    /// `[edit_start, edit_start + edit_len)` come from `edit_seed` instead.
    pub fn with_edit(seed: u64, len: u64, edit_start: u64, edit_len: u64, edit_seed: u64) -> Self {
        assert!(edit_start + edit_len <= len);
        Self {
            len,
            pos: 0,
            base_seed: seed,
            edit: Some((edit_start, edit_len, edit_seed)),
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

impl AsyncRead for DeterministicSource {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let remaining_total = this.len - this.pos;
        if remaining_total == 0 {
            return Poll::Ready(Ok(()));
        }
        let requested = buf.remaining() as u64;
        if requested == 0 {
            return Poll::Ready(Ok(()));
        }

        // Cap this fill at the end of the current seed region, so one
        // fill_range call never straddles base/edit content.
        let (seed, region_remaining) = match this.edit {
            Some((start, _, _)) if this.pos < start => (this.base_seed, start - this.pos),
            Some((start, len, edit_seed)) if this.pos < start + len => {
                (edit_seed, start + len - this.pos)
            }
            _ => (this.base_seed, remaining_total),
        };
        let take = requested.min(remaining_total).min(region_remaining) as usize;

        let dest = buf.initialize_unfilled_to(take);
        fill_range(seed, this.pos, &mut dest[..take]);
        buf.advance(take);
        this.pos += take as u64;
        Poll::Ready(Ok(()))
    }
}
