//! Intensity bucketing — turns a raw count into a 0..=4 level that
//! drives the cell's tailwind alpha. Pure functions.

use serde::{Deserialize, Serialize};

/// Five-level intensity scale matching GitHub's contribution graph.
/// `Zero` is no activity; `Four` is the brightest bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntensityBucket {
    Zero,
    One,
    Two,
    Three,
    Four,
}

impl IntensityBucket {
    /// Tailwind opacity suffix for `bg-{stem}-500/{N}`. Pairs with
    /// [`ColorTag::stem`] in the renderer.
    #[must_use]
    pub fn opacity(self) -> u8 {
        match self {
            Self::Zero => 0, // renders as a subtle neutral cell
            Self::One => 20,
            Self::Two => 40,
            Self::Three => 65,
            Self::Four => 90,
        }
    }
}

/// Accent color stem reused from view-kanban / view-calendar so a
/// project's palette stays consistent across views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorTag {
    Neutral,
    Primary,
    #[default]
    Success,
    Warning,
    Danger,
    Info,
}

impl ColorTag {
    #[must_use]
    pub fn stem(self) -> &'static str {
        match self {
            Self::Neutral => "slate",
            Self::Primary => "violet",
            Self::Success => "emerald",
            Self::Warning => "amber",
            Self::Danger => "rose",
            Self::Info => "sky",
        }
    }
}

/// Quartile-style bucketing. `max` is the largest value in the
/// dataset (passed in by the renderer once per layout). Empty
/// (`count == 0`) is always `Zero`; otherwise the count maps into
/// One..=Four by quartile of `max`.
#[must_use]
pub fn bucket_for(count: u32, max: u32) -> IntensityBucket {
    if count == 0 {
        return IntensityBucket::Zero;
    }
    if max == 0 {
        return IntensityBucket::One;
    }
    let q = (count as f32) / (max as f32);
    if q <= 0.25 {
        IntensityBucket::One
    } else if q <= 0.5 {
        IntensityBucket::Two
    } else if q <= 0.75 {
        IntensityBucket::Three
    } else {
        IntensityBucket::Four
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_count_is_zero_bucket() {
        assert_eq!(bucket_for(0, 10), IntensityBucket::Zero);
    }

    #[test]
    fn quartile_boundaries() {
        assert_eq!(bucket_for(1, 4), IntensityBucket::One);
        assert_eq!(bucket_for(2, 4), IntensityBucket::Two);
        assert_eq!(bucket_for(3, 4), IntensityBucket::Three);
        assert_eq!(bucket_for(4, 4), IntensityBucket::Four);
    }
}
