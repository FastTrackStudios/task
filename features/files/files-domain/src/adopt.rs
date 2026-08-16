//! Adoption — `files.adopt.*`.
//!
//! Most content does not arrive by upload. 6.1 TB of it is already on
//! disk, written by Pro Tools, Reaper and Resolve, and still being
//! written while we take it on.
//!
//! So adoption is a state machine with one governing rule: **structure
//! first, content addresses later.** Entries are published from what the
//! filesystem already knows — name, size, mtime — so a tree is browsable
//! within seconds whatever its size, and hashing runs behind that. An
//! entry without a verified address is *unverified*, not withheld.
//!
//! The other two rules fall out of that: interrupting loses only work in
//! flight, and a file modified while it is being hashed is re-hashed
//! rather than recorded wrongly. Neither ever blocks the application that
//! owns the tree.

use chrono::{DateTime, Utc};
use files_proto::service::roots::AdoptionPhase;

/// Why a hash was discarded rather than recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discarded {
    /// The file changed while we were reading it. Recording the address
    /// we computed would describe a file that never existed.
    ChangedUnderneath,
}

/// Progress through one adoption.
///
/// Counters only ever advance, which is what makes resumption cheap: a
/// resumed adoption re-does the work that was in flight and nothing
/// else.
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    phase: AdoptionPhase,
    /// Whether to read bytes at all. `false` publishes the catalogue and
    /// stops — for a tree being surveyed rather than taken on.
    hash_content: bool,
    entries_seen: u64,
    entries_hashed: u64,
    bytes_seen: u64,
    bytes_hashed: u64,
    /// Set only once the walk has finished, so a percentage is honest
    /// before then rather than inventing a denominator.
    entries_total: Option<u64>,
    started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Adoption {
    #[must_use]
    pub fn begin(now: DateTime<Utc>, hash_content: bool) -> Self {
        Self {
            phase: AdoptionPhase::Enumerating,
            hash_content,
            entries_seen: 0,
            entries_hashed: 0,
            bytes_seen: 0,
            bytes_hashed: 0,
            entries_total: None,
            started_at: now,
            updated_at: now,
        }
    }

    #[must_use]
    pub fn phase(&self) -> AdoptionPhase {
        self.phase
    }

    #[must_use]
    pub fn entries_seen(&self) -> u64 {
        self.entries_seen
    }

    #[must_use]
    pub fn entries_hashed(&self) -> u64 {
        self.entries_hashed
    }

    #[must_use]
    pub fn bytes_seen(&self) -> u64 {
        self.bytes_seen
    }

    #[must_use]
    pub fn bytes_hashed(&self) -> u64 {
        self.bytes_hashed
    }

    #[must_use]
    pub fn entries_total(&self) -> Option<u64> {
        self.entries_total
    }

    #[must_use]
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    #[must_use]
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Whether the tree can be browsed. True from the first published
    /// entry onward — the whole point of catalogue-first.
    #[must_use]
    pub fn is_browsable(&self) -> bool {
        self.entries_seen > 0
    }

    /// Whether adoption is still doing work.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(
            self.phase,
            AdoptionPhase::Enumerating | AdoptionPhase::Hashing
        )
    }

    /// An entry was published from filesystem metadata. Browsable
    /// immediately; its content address may not exist yet.
    pub fn saw(&mut self, size: u64, now: DateTime<Utc>) {
        if self.phase == AdoptionPhase::Paused {
            return;
        }
        self.entries_seen += 1;
        self.bytes_seen += size;
        self.updated_at = now;
    }

    /// The walk finished. Fixes the denominator, and decides whether
    /// there is a hashing phase at all.
    pub fn enumerated(&mut self, now: DateTime<Utc>) {
        if self.phase != AdoptionPhase::Enumerating {
            return;
        }
        self.entries_total = Some(self.entries_seen);
        self.updated_at = now;
        self.phase = if !self.hash_content || self.entries_seen == 0 {
            AdoptionPhase::Complete
        } else {
            AdoptionPhase::Hashing
        };
    }

    /// An entry's content address was computed and recorded.
    pub fn hashed(&mut self, size: u64, now: DateTime<Utc>) {
        if self.phase != AdoptionPhase::Hashing {
            return;
        }
        self.entries_hashed += 1;
        self.bytes_hashed += size;
        self.updated_at = now;
        if Some(self.entries_hashed) >= self.entries_total {
            self.phase = AdoptionPhase::Complete;
        }
    }

    /// A hash was thrown away because the file moved under us.
    ///
    /// Costs nothing but the work: the entry stays unverified and is
    /// picked up again, because recording an address for bytes that no
    /// longer exist is worse than not having one.
    pub fn discarded(&mut self, _why: Discarded, now: DateTime<Utc>) {
        self.updated_at = now;
    }

    /// Stop, leaving everything published so far browsable.
    pub fn pause(&mut self, now: DateTime<Utc>) {
        if self.is_running() {
            self.phase = AdoptionPhase::Paused;
            self.updated_at = now;
        }
    }

    /// Continue from where it stopped. Never restarts — counters were
    /// preserved across the pause, so resumption picks up the remainder.
    pub fn resume(&mut self, now: DateTime<Utc>) {
        if self.phase != AdoptionPhase::Paused {
            return;
        }
        self.updated_at = now;
        self.phase = match self.entries_total {
            None => AdoptionPhase::Enumerating,
            Some(total) if self.hash_content && self.entries_hashed < total => {
                AdoptionPhase::Hashing
            }
            Some(_) => AdoptionPhase::Complete,
        };
    }

    /// How much is left to hash. `None` until the walk has finished,
    /// because before that the answer is unknown rather than zero.
    #[must_use]
    pub fn remaining(&self) -> Option<u64> {
        self.entries_total
            .map(|total| total.saturating_sub(self.entries_hashed))
    }

    /// Fraction verified, 0.0–1.0.
    ///
    /// `None` while enumerating. A progress bar that invents a
    /// denominator mid-walk runs backwards, and on a 14,671-file album
    /// that is what a user remembers.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        match self.entries_total {
            None => None,
            Some(0) => Some(1.0),
            #[allow(clippy::cast_precision_loss)]
            Some(total) => Some((self.entries_hashed as f32 / total as f32).min(1.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn walked(a: &mut Adoption, n: u64) {
        for _ in 0..n {
            a.saw(1_000, at(1));
        }
    }

    #[test]
    fn browsable_before_anything_is_hashed() {
        let mut a = Adoption::begin(at(0), true);
        assert!(!a.is_browsable());
        a.saw(4_000_000_000, at(1));
        assert!(a.is_browsable(), "structure first is the whole point");
        assert_eq!(a.phase(), AdoptionPhase::Enumerating);
        assert_eq!(a.entries_hashed(), 0);
    }

    #[test]
    fn no_denominator_until_the_walk_finishes() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 500);
        assert_eq!(a.fraction(), None, "a bar that guesses runs backwards");
        assert_eq!(a.remaining(), None);
        a.enumerated(at(2));
        assert_eq!(a.entries_total(), Some(500));
        assert_eq!(a.fraction(), Some(0.0));
        assert_eq!(a.remaining(), Some(500));
    }

    #[test]
    fn survey_only_skips_hashing_entirely() {
        let mut a = Adoption::begin(at(0), false);
        walked(&mut a, 10);
        a.enumerated(at(2));
        assert_eq!(a.phase(), AdoptionPhase::Complete);
        assert_eq!(a.entries_hashed(), 0);
    }

    #[test]
    fn completes_when_every_entry_is_verified() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 3);
        a.enumerated(at(2));
        for i in 0..3 {
            a.hashed(1_000, at(3 + i));
        }
        assert_eq!(a.phase(), AdoptionPhase::Complete);
        assert_eq!(a.fraction(), Some(1.0));
    }

    #[test]
    fn resuming_continues_and_does_not_restart() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 100);
        a.enumerated(at(2));
        for i in 0..40 {
            a.hashed(1_000, at(3 + i));
        }
        a.pause(at(100));
        assert_eq!(a.phase(), AdoptionPhase::Paused);
        assert!(a.is_browsable(), "a paused tree stays browsable");

        a.resume(at(200));
        assert_eq!(a.phase(), AdoptionPhase::Hashing);
        assert_eq!(a.entries_hashed(), 40, "40 hashes were not thrown away");
        assert_eq!(a.remaining(), Some(60));
    }

    #[test]
    fn pausing_mid_walk_resumes_into_the_walk() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 7);
        a.pause(at(10));
        a.resume(at(20));
        assert_eq!(a.phase(), AdoptionPhase::Enumerating);
        assert_eq!(a.entries_seen(), 7);
    }

    #[test]
    fn a_paused_adoption_records_nothing_further() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 5);
        a.pause(at(10));
        a.saw(1_000, at(11));
        assert_eq!(a.entries_seen(), 5);
    }

    #[test]
    fn a_file_changing_under_us_costs_only_the_work() {
        let mut a = Adoption::begin(at(0), true);
        walked(&mut a, 2);
        a.enumerated(at(2));
        a.discarded(Discarded::ChangedUnderneath, at(3));
        assert_eq!(a.entries_hashed(), 0, "unverified, not wrongly verified");
        assert_eq!(a.phase(), AdoptionPhase::Hashing, "and never blocked");
        a.hashed(1_000, at(4));
        a.hashed(1_000, at(5));
        assert_eq!(a.phase(), AdoptionPhase::Complete);
    }

    #[test]
    fn an_empty_tree_completes() {
        let mut a = Adoption::begin(at(0), true);
        a.enumerated(at(1));
        assert_eq!(a.phase(), AdoptionPhase::Complete);
        assert_eq!(a.fraction(), Some(1.0));
    }
}
