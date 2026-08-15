//! Per-project state registry — custom status names with
//! canonical *state groups* (Plane's `models/state.py` insight).
//!
//! `TaskInfo::status` stays a free string (the vault is
//! hand-editable); this module only adds **meaning**: every
//! status name resolves to one of five [`StateGroup`]s, and
//! burndown / rollups / kanban classification work over the
//! groups instead of string-matching status names. A project may
//! declare its own registry in frontmatter (`states:` — see
//! [`StatesConfig`]); when absent, [`default_states`] mirrors
//! today's canonical statuses.
//!
//! ## Default group mapping
//!
//! | status        | group       |
//! |---------------|-------------|
//! | `triage`      | `backlog`   |
//! | `open`        | `unstarted` |
//! | `in-progress` | `started`   |
//! | `waiting`     | `started`   |
//! | `done`        | `completed` |
//! | `cancelled`   | `cancelled` |
//!
//! `waiting` maps to **started** (not backlog): a waiting task
//! is claimed work that's paused on an external dependency, so
//! it belongs to the in-flight bucket for burndown purposes.
//!
//! ## Fallback
//!
//! [`resolve_state_group`] is alias-tolerant (`in_progress` /
//! `In Progress` / `doing` all match) and resolves any status
//! it can't place — custom names absent from the registry —
//! to [`StateGroup::Unstarted`]: unknown work is presumed not
//! yet begun, which keeps progress percentages conservative.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// The five canonical state groups (mirrors Plane). Custom
/// status names map onto these; *all* progress classification
/// (rollups, burndown, kanban column buckets, `issue stats`)
/// goes through the group, never the raw status string.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum StateGroup {
    /// Not yet triaged / groomed into actionable work.
    Backlog,
    /// Actionable but not begun. Also the documented fallback
    /// for unknown status names.
    Unstarted,
    /// Claimed / in-flight work (includes `waiting` — paused
    /// but claimed).
    Started,
    /// Finished successfully. The only group that counts as
    /// "done" in progress ratios.
    Completed,
    /// Closed without delivery. Closed for blocker-resolution
    /// purposes, but not "progress".
    Cancelled,
}

impl StateGroup {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Unstarted => "unstarted",
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse a group slug. Alias-tolerant (`complete` /
    /// `canceled` accepted); `None` for anything else.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match normalize(s).as_str() {
            "backlog" => Self::Backlog,
            "unstarted" => Self::Unstarted,
            "started" => Self::Started,
            "completed" | "complete" => Self::Completed,
            "cancelled" | "canceled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// `true` once work under this group no longer counts as
    /// open — the blocker-resolution rule (`done` *or*
    /// `cancelled` unblocks dependents).
    #[must_use]
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// One registered state: a project-local status name bound to
/// its canonical [`StateGroup`].
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct StateDef {
    /// The status name as written in task frontmatter
    /// (`status: qa-review`). Matching is alias-tolerant
    /// (case / `_` vs `-` / whitespace insensitive).
    pub name: String,
    /// Canonical group this state classifies into.
    pub group: StateGroup,
    /// Hex `#RRGGBB` for UI pills / columns. Empty = UI picks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// `true` for the state newly-created tasks default to.
    /// At most one per registry (first wins on conflict).
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    /// Sort position for boards / pickers (ascending).
    #[serde(default)]
    pub order: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The project's state registry — the optional `states:` list in
/// project frontmatter. `Vec<StateDef>` newtype so architect can
/// store it as a JSON column (same pattern as `Tags`).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StatesConfig(pub Vec<StateDef>);

impl StatesConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The registered default state name, if any (the state a
    /// freshly created task starts in). First `default: true`
    /// wins.
    #[must_use]
    pub fn default_state(&self) -> Option<&StateDef> {
        self.0.iter().find(|s| s.default)
    }

    /// States in display order (`order` ascending, declaration
    /// order as the tiebreak).
    #[must_use]
    pub fn ordered(&self) -> Vec<&StateDef> {
        let mut v: Vec<&StateDef> = self.0.iter().collect();
        v.sort_by_key(|s| s.order);
        v
    }
}

impl From<Vec<StateDef>> for StatesConfig {
    fn from(v: Vec<StateDef>) -> Self {
        Self(v)
    }
}

/// The registry used when a project declares no `states:` config
/// — mirrors today's canonical statuses (see the module docs for
/// the table + the `waiting` → `started` rationale).
#[must_use]
pub fn default_states() -> StatesConfig {
    let def = |name: &str, group: StateGroup, default: bool, order: u32| StateDef {
        name: name.into(),
        group,
        color: String::new(),
        default,
        order,
    };
    StatesConfig(vec![
        def("triage", StateGroup::Backlog, false, 0),
        def("open", StateGroup::Unstarted, true, 1),
        def("in-progress", StateGroup::Started, false, 2),
        def("waiting", StateGroup::Started, false, 3),
        def("done", StateGroup::Completed, false, 4),
        def("cancelled", StateGroup::Cancelled, false, 5),
    ])
}

/// Case / separator / whitespace-insensitive form used for all
/// status-name matching: trimmed, lowercased, `_` and spaces
/// collapsed to `-`.
fn normalize(s: &str) -> String {
    s.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c == '_' || c.is_whitespace() {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Resolve a raw status string to its [`StateGroup`].
///
/// Resolution order:
/// 1. the project's own registry (`states:` frontmatter), when
///    present — alias-tolerant name match;
/// 2. the built-in alias table (canonical statuses + their
///    common spellings, e.g. `todo` → unstarted, `blocked` →
///    started, `shipped` → completed);
/// 3. **fallback**: [`StateGroup::Unstarted`] — an unknown
///    status is presumed not-yet-begun, keeping progress
///    ratios conservative. Documented contract; don't "fix"
///    this to error.
#[must_use]
pub fn resolve_state_group(states: Option<&StatesConfig>, status: &str) -> StateGroup {
    let norm = normalize(status);
    if let Some(cfg) = states {
        if let Some(def) = cfg.0.iter().find(|d| normalize(&d.name) == norm) {
            return def.group;
        }
    }
    builtin_group(&norm).unwrap_or(StateGroup::Unstarted)
}

/// Built-in alias table over the canonical status set. Input
/// must already be [`normalize`]d.
fn builtin_group(norm: &str) -> Option<StateGroup> {
    Some(match norm {
        "triage" | "backlog" => StateGroup::Backlog,
        "open" | "todo" | "none" | "unstarted" => StateGroup::Unstarted,
        "in-progress" | "doing" | "started" | "active" | "waiting" | "blocked" => {
            StateGroup::Started
        }
        "done" | "completed" | "complete" | "shipped" => StateGroup::Completed,
        "cancelled" | "canceled" | "abandoned" => StateGroup::Cancelled,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_mirrors_canonical_statuses() {
        let cfg = default_states();
        let group = |name: &str| resolve_state_group(Some(&cfg), name);
        assert_eq!(group("triage"), StateGroup::Backlog);
        assert_eq!(group("open"), StateGroup::Unstarted);
        assert_eq!(group("in-progress"), StateGroup::Started);
        assert_eq!(group("waiting"), StateGroup::Started, "claimed work");
        assert_eq!(group("done"), StateGroup::Completed);
        assert_eq!(group("cancelled"), StateGroup::Cancelled);
        assert_eq!(cfg.default_state().map(|d| d.name.as_str()), Some("open"));
    }

    #[test]
    fn resolution_is_alias_tolerant() {
        // No config: built-in aliases.
        assert_eq!(
            resolve_state_group(None, "In_Progress"),
            StateGroup::Started
        );
        assert_eq!(resolve_state_group(None, " DONE "), StateGroup::Completed);
        assert_eq!(resolve_state_group(None, "canceled"), StateGroup::Cancelled);
        // Custom config: name matching tolerates case + `_`.
        let cfg = StatesConfig(vec![StateDef {
            name: "QA Review".into(),
            group: StateGroup::Started,
            color: String::new(),
            default: false,
            order: 0,
        }]);
        assert_eq!(
            resolve_state_group(Some(&cfg), "qa_review"),
            StateGroup::Started
        );
    }

    #[test]
    fn custom_registry_wins_over_builtin_aliases() {
        // A project may re-bind a canonical name — e.g. treat
        // `waiting` as backlog. Registry beats the alias table.
        let cfg = StatesConfig(vec![StateDef {
            name: "waiting".into(),
            group: StateGroup::Backlog,
            color: String::new(),
            default: false,
            order: 0,
        }]);
        assert_eq!(
            resolve_state_group(Some(&cfg), "waiting"),
            StateGroup::Backlog
        );
        // Unlisted canonical names still fall through to builtins.
        assert_eq!(
            resolve_state_group(Some(&cfg), "done"),
            StateGroup::Completed
        );
    }

    #[test]
    fn unknown_status_falls_back_to_unstarted() {
        assert_eq!(
            resolve_state_group(None, "warp-speed"),
            StateGroup::Unstarted
        );
        let cfg = default_states();
        assert_eq!(
            resolve_state_group(Some(&cfg), "qa-review"),
            StateGroup::Unstarted
        );
    }

    #[test]
    fn states_config_round_trips_yaml() {
        let cfg = StatesConfig(vec![
            StateDef {
                name: "specced".into(),
                group: StateGroup::Unstarted,
                color: "#88ccff".into(),
                default: true,
                order: 1,
            },
            StateDef {
                name: "building".into(),
                group: StateGroup::Started,
                color: String::new(),
                default: false,
                order: 2,
            },
        ]);
        let yaml = serde_yaml::to_string(&cfg).expect("serialize");
        // Empty/false optional fields stay out of the frontmatter.
        assert!(!yaml.contains("color: ''"));
        assert!(!yaml.contains("default: false"));
        let back: StatesConfig = serde_yaml::from_str(&yaml).expect("parse");
        assert_eq!(back, cfg);
    }

    #[test]
    fn ordered_sorts_by_order() {
        let mut cfg = default_states();
        cfg.0.reverse();
        let names: Vec<&str> = cfg.ordered().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "triage",
                "open",
                "in-progress",
                "waiting",
                "done",
                "cancelled"
            ]
        );
    }
}
