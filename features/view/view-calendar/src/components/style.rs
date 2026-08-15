//! Static class mappings for [`ColorTag`].
//!
//! Tailwind v4 only inlines classes it sees as *literal* strings in
//! source. Constructing `bg-violet-500/90` via `format!()` would
//! silently drop the rule. We map each [`ColorTag`] to a hard-coded
//! class set so every variant the calendar can emit is literally
//! present in this file — and therefore in the compiled CSS.
//!
//! Visuals follow Google Calendar / Apple Calendar conventions:
//! solid colored body, brighter left accent bar, hover ring.

use crate::types::ColorTag;

/// Class set used for time-grid event chips (week/day) and month-view
/// chips:
///
/// - `body`  — fill + text color + accent border
/// - `hover` — hover-only emphasis (ring + brighten)
#[derive(Clone, Copy, Debug)]
pub struct ChipPalette {
    pub body: &'static str,
    pub hover: &'static str,
}

#[must_use]
pub fn chip_palette(color: ColorTag) -> ChipPalette {
    match color {
        ColorTag::Neutral => ChipPalette {
            body: "bg-slate-600/90 text-white border-l-4 border-slate-300",
            hover: "hover:bg-slate-600 hover:ring-1 hover:ring-slate-300/60",
        },
        ColorTag::Primary => ChipPalette {
            body: "bg-violet-600/90 text-white border-l-4 border-violet-300",
            hover: "hover:bg-violet-600 hover:ring-1 hover:ring-violet-300/60",
        },
        ColorTag::Success => ChipPalette {
            body: "bg-emerald-600/90 text-white border-l-4 border-emerald-300",
            hover: "hover:bg-emerald-600 hover:ring-1 hover:ring-emerald-300/60",
        },
        ColorTag::Warning => ChipPalette {
            body: "bg-amber-500/90 text-amber-50 border-l-4 border-amber-200",
            hover: "hover:bg-amber-500 hover:ring-1 hover:ring-amber-200/60",
        },
        ColorTag::Danger => ChipPalette {
            body: "bg-rose-600/90 text-white border-l-4 border-rose-300",
            hover: "hover:bg-rose-600 hover:ring-1 hover:ring-rose-300/60",
        },
        ColorTag::Info => ChipPalette {
            body: "bg-sky-600/90 text-white border-l-4 border-sky-300",
            hover: "hover:bg-sky-600 hover:ring-1 hover:ring-sky-300/60",
        },
    }
}

/// Class set for a day-plan "ghost" block — a faint, transparent
/// colored panel with a thin left accent bar that sits behind real
/// events. Same literal-class requirement as [`chip_palette`]: every
/// variant is spelled out so Tailwind keeps the rules.
#[must_use]
pub fn template_palette(color: ColorTag) -> &'static str {
    match color {
        ColorTag::Neutral => "bg-slate-500/10 border-l-2 border-slate-400/60 text-slate-200/90",
        ColorTag::Primary => "bg-violet-500/10 border-l-2 border-violet-400/60 text-violet-100/90",
        ColorTag::Success => {
            "bg-emerald-500/10 border-l-2 border-emerald-400/60 text-emerald-100/90"
        }
        ColorTag::Warning => "bg-amber-500/10 border-l-2 border-amber-400/60 text-amber-100/90",
        ColorTag::Danger => "bg-rose-500/10 border-l-2 border-rose-400/60 text-rose-100/90",
        ColorTag::Info => "bg-sky-500/10 border-l-2 border-sky-400/60 text-sky-100/90",
    }
}
