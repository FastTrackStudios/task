//! FSRS — the Free Spaced Repetition Scheduler.
//!
//! A faithful, dependency-light implementation of the modern SM-2
//! successor (FSRS-5, the algorithm Anki ships). It is a *pure*
//! scheduler: give it a card's memory state and a [`Rating`], and it
//! returns the next state — updated **stability** (days until
//! retrievability decays to ~90%) and **difficulty** (1–10, how hard
//! the item is for you), plus the next `due` date derived from your
//! **desired retention**.
//!
//! Deliberately clock-free and storage-free — the caller passes `today`
//! and stores the returned [`FsrsCard`]. The `recall` feature persists
//! these fields as `sr-*` frontmatter keys, exactly the way the inbox's
//! SM-2 scheduler rode in an item's frontmatter.
//!
//! ## The model
//!
//! Retrievability after `t` days with stability `S`:
//!
//! ```text
//! R(t, S) = (1 + FACTOR · t/S)^DECAY
//! ```
//!
//! with `DECAY = -0.5` and `FACTOR = 19/81` (so `R = 0.9` exactly when
//! `t = S`). The next interval solves `R(I, S) = desired_retention`:
//!
//! ```text
//! I = (S / FACTOR) · (desired_retention^(1/DECAY) − 1)
//! ```
//!
//! Stability and difficulty updates follow the published FSRS-5
//! formulas (see [`Weights`]).

use chrono::{Duration, NaiveDate};

/// How well the reviewer recalled the card. The four-button grading
/// FSRS/Anki use; the discriminants are the FSRS grades `G ∈ 1..=4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Rating {
    /// Total blackout / wrong — a lapse. Resets stability sharply.
    Again = 1,
    /// Recalled with serious difficulty.
    Hard = 2,
    /// Recalled correctly with some effort — the "normal" success.
    Good = 3,
    /// Recalled effortlessly.
    Easy = 4,
}

impl Rating {
    /// The FSRS grade `G ∈ 1..=4`.
    #[must_use]
    pub const fn grade(self) -> f64 {
        self as u8 as f64
    }

    /// Parse a persisted / UI grade (`1..=4`); anything else → `None`.
    #[must_use]
    pub const fn from_grade(g: u8) -> Option<Self> {
        match g {
            1 => Some(Self::Again),
            2 => Some(Self::Hard),
            3 => Some(Self::Good),
            4 => Some(Self::Easy),
            _ => None,
        }
    }
}

/// The 19 FSRS-5 model weights. `w[0..=3]` are the initial stabilities
/// for the four first-review grades; the rest drive the
/// stability/difficulty update formulas.
pub type Weights = [f64; 19];

/// FSRS-5 default weights (the parameters Anki ships before per-user
/// optimization). Good enough for a general deck; a future optimizer
/// can fit these to review history.
pub const DEFAULT_WEIGHTS: Weights = [
    0.402_55, 1.183_85, 3.173, 15.691_05, 7.194_9, 0.534_5, 1.460_4, 0.004_6, 1.545_75, 0.119_2,
    1.019_25, 1.939_5, 0.11, 0.296_05, 2.269_8, 0.231_5, 2.989_8, 0.516_55, 0.662_1,
];

/// Power-law forgetting-curve decay. `R = 0.9` at `t = S`.
const DECAY: f64 = -0.5;
/// Curve factor tied to `DECAY` so that `R(S, S) = 0.9`.
const FACTOR: f64 = 19.0 / 81.0;

/// Stability never drops below this (days) — keeps intervals finite.
const MIN_STABILITY: f64 = 0.01;

/// One learning card's FSRS memory state. All scheduler-owned; the
/// front/back/prompt live on the domain entity that embeds these.
#[derive(Debug, Clone, PartialEq)]
pub struct FsrsCard {
    /// Memory stability in days — the interval at which retrievability
    /// has decayed to ~90%. `0.0` for a brand-new, never-reviewed card.
    pub stability: f64,
    /// Difficulty `∈ [1, 10]` — how intrinsically hard the item is.
    /// `0.0` for a never-reviewed card (seeded on first review).
    pub difficulty: f64,
    /// Total number of reviews.
    pub reps: i64,
    /// Number of `Again` lapses.
    pub lapses: i64,
    /// Date of the most recent review, if any.
    pub last_review: Option<NaiveDate>,
    /// Next scheduled review date, if any.
    pub due: Option<NaiveDate>,
}

impl Default for FsrsCard {
    fn default() -> Self {
        Self::new()
    }
}

impl FsrsCard {
    /// A fresh, never-reviewed card. The first [`review`] seeds its
    /// stability + difficulty from the grade given.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stability: 0.0,
            difficulty: 0.0,
            reps: 0,
            lapses: 0,
            last_review: None,
            due: None,
        }
    }

    /// True if the card has never been reviewed (needs first-review
    /// seeding rather than the update formulas).
    #[must_use]
    pub fn is_new(&self) -> bool {
        self.reps == 0 || self.last_review.is_none() || self.stability <= 0.0
    }

    /// Due for review on or before `today`. A never-scheduled card is
    /// always due.
    #[must_use]
    pub fn is_due(&self, today: NaiveDate) -> bool {
        self.due.is_none_or(|d| d <= today)
    }
}

/// Review a card with the default FSRS-5 weights. See [`review_with`].
#[must_use]
pub fn review(
    card: &FsrsCard,
    rating: Rating,
    today: NaiveDate,
    desired_retention: f64,
) -> FsrsCard {
    review_with(card, rating, today, desired_retention, &DEFAULT_WEIGHTS)
}

/// Apply one `rating` to `card` on `today`, targeting `desired_retention`
/// (e.g. `0.9`), and return the updated card. Pure — no clock, no I/O.
///
/// A never-reviewed card is *seeded* (initial stability = `w[grade-1]`,
/// initial difficulty from `w[4]`/`w[5]`). An existing card runs the
/// FSRS-5 stability/difficulty update off its retrievability at review
/// time (how much was forgettable since `last_review`).
#[must_use]
pub fn review_with(
    card: &FsrsCard,
    rating: Rating,
    today: NaiveDate,
    desired_retention: f64,
    w: &Weights,
) -> FsrsCard {
    let retention = desired_retention.clamp(0.01, 0.99);

    let (stability, difficulty) = if card.is_new() {
        (init_stability(w, rating), init_difficulty(w, rating))
    } else {
        let elapsed = card
            .last_review
            .map_or(0, |last| (today - last).num_days().max(0));
        let r = retrievability(elapsed, card.stability);
        let d = next_difficulty(w, card.difficulty, rating);
        let s = if rating == Rating::Again {
            next_forget_stability(w, card.difficulty, card.stability, r)
        } else {
            next_recall_stability(w, d, card.stability, r, rating)
        };
        (s, d)
    };

    let stability = stability.max(MIN_STABILITY);
    let interval = next_interval(stability, retention);

    FsrsCard {
        stability,
        difficulty: difficulty.clamp(1.0, 10.0),
        reps: card.reps + 1,
        lapses: card.lapses + i64::from(rating == Rating::Again),
        last_review: Some(today),
        due: Some(today + Duration::days(interval)),
    }
}

/// Days until `stability` decays to `desired_retention`. At least 1.
#[must_use]
pub fn next_interval(stability: f64, desired_retention: f64) -> i64 {
    let raw = (stability / FACTOR) * (desired_retention.powf(1.0 / DECAY) - 1.0);
    (raw.round() as i64).max(1)
}

/// Retrievability of a memory with `stability` after `elapsed_days`.
#[must_use]
pub fn retrievability(elapsed_days: i64, stability: f64) -> f64 {
    if stability <= 0.0 {
        return 0.0;
    }
    let t = elapsed_days.max(0) as f64;
    (1.0 + FACTOR * t / stability).powf(DECAY)
}

// ── FSRS-5 update formulas ──────────────────────────────────────────

fn init_stability(w: &Weights, rating: Rating) -> f64 {
    let idx = (rating as usize) - 1; // Again→0 … Easy→3
    w[idx].max(MIN_STABILITY)
}

fn init_difficulty(w: &Weights, rating: Rating) -> f64 {
    // D_0(G) = w4 − e^(w5·(G−1)) + 1
    (w[4] - (w[5] * (rating.grade() - 1.0)).exp() + 1.0).clamp(1.0, 10.0)
}

fn next_difficulty(w: &Weights, difficulty: f64, rating: Rating) -> f64 {
    // Linear-damped delta, then mean-revert toward the "Easy" seed.
    let delta = -w[6] * (rating.grade() - 3.0);
    let damped = difficulty + delta * (10.0 - difficulty) / 9.0;
    let target = init_difficulty(w, Rating::Easy);
    (w[7] * target + (1.0 - w[7]) * damped).clamp(1.0, 10.0)
}

fn next_recall_stability(w: &Weights, d: f64, s: f64, r: f64, rating: Rating) -> f64 {
    let hard_penalty = if rating == Rating::Hard { w[15] } else { 1.0 };
    let easy_bonus = if rating == Rating::Easy { w[16] } else { 1.0 };
    let growth = w[8].exp()
        * (11.0 - d)
        * s.powf(-w[9])
        * ((w[10] * (1.0 - r)).exp() - 1.0)
        * hard_penalty
        * easy_bonus;
    s * (1.0 + growth)
}

fn next_forget_stability(w: &Weights, d: f64, s: f64, r: f64) -> f64 {
    let forget = w[11] * d.powf(-w[12]) * ((s + 1.0).powf(w[13]) - 1.0) * (w[14] * (1.0 - r)).exp();
    // A lapse must not *raise* stability.
    forget.min(s).max(MIN_STABILITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn new_card_seeds_per_rating() {
        let today = day("2026-07-16");
        let base = FsrsCard::new();
        let again = review(&base, Rating::Again, today, 0.9);
        let hard = review(&base, Rating::Hard, today, 0.9);
        let good = review(&base, Rating::Good, today, 0.9);
        let easy = review(&base, Rating::Easy, today, 0.9);

        // Initial stability strictly increases with the grade.
        assert!(again.stability < hard.stability);
        assert!(hard.stability < good.stability);
        assert!(good.stability < easy.stability);

        // Every first review counts + sets last_review/due.
        for c in [&again, &hard, &good, &easy] {
            assert_eq!(c.reps, 1);
            assert_eq!(c.last_review, Some(today));
            assert!(c.due.unwrap() >= today);
            assert!((1.0..=10.0).contains(&c.difficulty));
        }
        assert_eq!(again.lapses, 1);
        assert_eq!(good.lapses, 0);

        // Easier grade ⇒ longer first interval.
        assert!(again.due.unwrap() <= good.due.unwrap());
        assert!(good.due.unwrap() <= easy.due.unwrap());
    }

    #[test]
    fn repeated_good_grows_interval() {
        let mut today = day("2026-07-16");
        let mut card = review(&FsrsCard::new(), Rating::Good, today, 0.9);
        let mut last_interval = (card.due.unwrap() - today).num_days();

        for _ in 0..5 {
            today = card.due.unwrap();
            card = review(&card, Rating::Good, today, 0.9);
            let interval = (card.due.unwrap() - today).num_days();
            assert!(
                interval >= last_interval,
                "interval should not shrink under repeated Good: {last_interval} → {interval}"
            );
            last_interval = interval;
        }
        // After several successful reviews the interval is clearly > 1.
        assert!(last_interval > 1);
    }

    #[test]
    fn again_resets_stability() {
        let mut today = day("2026-07-16");
        let mut card = review(&FsrsCard::new(), Rating::Good, today, 0.9);
        for _ in 0..3 {
            today = card.due.unwrap();
            card = review(&card, Rating::Good, today, 0.9);
        }
        let strong = card.stability;
        let strong_interval = (card.due.unwrap() - today).num_days();

        today = card.due.unwrap();
        let lapsed = review(&card, Rating::Again, today, 0.9);

        assert!(
            lapsed.stability < strong,
            "Again must lower stability: {strong} → {}",
            lapsed.stability
        );
        assert_eq!(lapsed.lapses, card.lapses + 1);
        let lapsed_interval = (lapsed.due.unwrap() - today).num_days();
        assert!(
            lapsed_interval < strong_interval,
            "Again must shorten the interval"
        );
    }

    #[test]
    fn higher_retention_shortens_interval() {
        let today = day("2026-07-16");
        let card = review(&FsrsCard::new(), Rating::Good, today, 0.9);
        let low = review(&card, Rating::Good, card.due.unwrap(), 0.80);
        let high = review(&card, Rating::Good, card.due.unwrap(), 0.95);
        let low_i = (low.due.unwrap() - card.due.unwrap()).num_days();
        let high_i = (high.due.unwrap() - card.due.unwrap()).num_days();
        assert!(
            high_i < low_i,
            "wanting to remember MORE ⇒ review sooner: 0.95→{high_i}d vs 0.80→{low_i}d"
        );
    }

    #[test]
    fn interval_solves_the_curve() {
        // At t = interval, retrievability should be ~ desired retention.
        let s = 20.0;
        let i = next_interval(s, 0.9);
        let r = retrievability(i, s);
        assert!((r - 0.9).abs() < 0.05, "R({i}) = {r}, expected ≈0.9");
    }
}
