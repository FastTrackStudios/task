//! `task-plugin` — the plugin vocabulary.
//!
//! The core Task model (tasks, projects, the vault, orgs, auth) is the
//! platform; everything domain-flavoured is a **plugin** an org can
//! turn on and off. This crate is the shared language for that:
//!
//! - [`PluginInfo`] — identity: id, display name, whether it is core.
//! - [`catalog`] — the known plugins, one authoritative list.
//! - [`PluginSet`] — the resolution from an org's manifest entry to
//!   the effective enabled set.
//!
//! What a plugin *contributes* is deliberately not defined here. The
//! server and the UI are different binaries with different targets, so
//! contribution types live with their surfaces (service mounts with
//! the server, nav/widgets/stores with `task-ui-core`) and key off the
//! ids defined here. This crate stays wasm-clean and dependency-light
//! so every surface — server, wasm UI, CLI — can share it.
//!
//! Design: apps/task/plans/plugin-system.md.

use serde::{Deserialize, Serialize};

/// Identity and metadata for one plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginInfo {
    /// Stable machine id — what `org.toml` stores and surfaces key on.
    /// Lowercase ASCII + `-`, never renamed once shipped.
    pub id: &'static str,
    /// Human name for settings surfaces.
    pub name: &'static str,
    /// One-line description for settings surfaces.
    pub description: &'static str,
    /// Core plugins are the platform: always enabled, not toggleable,
    /// and not listed in `org.toml`. Kept in the catalog so every
    /// surface can render a complete picture.
    pub core: bool,
}

/// The authoritative list of plugins this build knows about.
///
/// Grouping rationale lives in the plan doc. Order is display order.
pub const CATALOG: &[PluginInfo] = &[
    PluginInfo {
        id: "core",
        name: "Task core",
        description: "Tasks, projects, goals, milestones, the vault, orgs and sharing",
        core: true,
    },
    PluginInfo {
        id: "mealplan",
        name: "Meal planning",
        description: "Recipes, pantry stock, meal plans and shopping lists",
        core: false,
    },
    PluginInfo {
        id: "fitness",
        name: "Fitness",
        description: "Body metrics, exercises, workouts and nutrition logging",
        core: false,
    },
    PluginInfo {
        id: "fasttrackstudio",
        name: "FastTrackStudio",
        description: "Song and setlist embeds, the session player, chart surfaces",
        core: false,
    },
    PluginInfo {
        id: "wiki",
        name: "Wiki",
        description: "The LLM-maintained knowledge wiki and its graph",
        core: false,
    },
    PluginInfo {
        id: "scripture",
        name: "Scripture",
        description: "Bible references, reading and study panels",
        core: false,
    },
    PluginInfo {
        id: "email",
        name: "Email",
        description: "Synced mail accounts and the reader",
        core: false,
    },
    PluginInfo {
        id: "forge",
        name: "Forges",
        description: "GitHub / Forgejo issues, pull requests and repo links",
        core: false,
    },
    PluginInfo {
        id: "agent",
        name: "Agents",
        description: "LLM agent sessions, routines and the agent board",
        core: false,
    },
    PluginInfo {
        id: "scheduling",
        name: "Scheduling",
        description: "Day plans, calendar events and bookable slots",
        core: false,
    },
    PluginInfo {
        id: "finance",
        name: "Finance",
        description: "Invoicing, ledgers and billing reports",
        core: false,
    },
    PluginInfo {
        id: "contacts",
        name: "Contacts",
        description: "People records and rosters",
        core: false,
    },
    PluginInfo {
        id: "recall",
        name: "Recall",
        description: "Spaced-repetition cards and review queues",
        core: false,
    },
    PluginInfo {
        id: "files",
        name: "Files",
        description: "File Roots, versioned project folders, the Drive surface and the file explorer",
        core: false,
    },
    PluginInfo {
        id: "home",
        name: "Home ops",
        description: "Locations and physical inventory",
        core: false,
    },
];

/// Look a plugin up by id.
#[must_use]
pub fn find(id: &str) -> Option<&'static PluginInfo> {
    CATALOG.iter().find(|p| p.id == id)
}

/// How an org's manifest expresses its plugin choices.
///
/// Absent (`None` in the manifest) means "everything on" — the
/// pre-plugin behaviour, and the right default for existing orgs whose
/// `org.toml` predates this field. `Enabled` is an allow-list (core is
/// implicitly included); `Disabled` is a deny-list. One or the other:
/// a combined form invites contradictions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginChoice {
    /// Exactly these plugins (plus core, always).
    Enabled(Vec<String>),
    /// Everything except these.
    Disabled(Vec<String>),
}

/// The effective enabled set for one org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSet {
    enabled: Vec<&'static str>,
}

impl PluginSet {
    /// Resolve a manifest entry against the catalog.
    ///
    /// Unknown ids are logged and ignored rather than erroring: an
    /// `org.toml` written by a newer build must still load on an older
    /// one, and a typo in a deny-list should not take the org down.
    /// Core plugins are always in the result regardless of the choice.
    #[must_use]
    pub fn resolve(choice: Option<&PluginChoice>) -> Self {
        let warn_unknown = |ids: &[String]| {
            for id in ids {
                if find(id).is_none() {
                    tracing::warn!(plugin = %id, "org.toml names an unknown plugin — ignored");
                }
            }
        };
        let enabled = match choice {
            None => CATALOG.iter().map(|p| p.id).collect(),
            Some(PluginChoice::Enabled(ids)) => {
                warn_unknown(ids);
                CATALOG
                    .iter()
                    .filter(|p| p.core || ids.iter().any(|i| i == p.id))
                    .map(|p| p.id)
                    .collect()
            }
            Some(PluginChoice::Disabled(ids)) => {
                warn_unknown(ids);
                CATALOG
                    .iter()
                    .filter(|p| p.core || !ids.iter().any(|i| i == p.id))
                    .map(|p| p.id)
                    .collect()
            }
        };
        Self { enabled }
    }

    /// Is `id` on for this org? Unknown ids are off — a surface asking
    /// about a plugin this build doesn't know can't serve it anyway.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.enabled.contains(&id)
    }

    /// The enabled ids, catalog order.
    #[must_use]
    pub fn ids(&self) -> &[&'static str] {
        &self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for p in CATALOG {
            assert!(seen.insert(p.id), "duplicate plugin id {}", p.id);
            assert!(
                p.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "id {} is not lowercase-ascii-dashed",
                p.id
            );
            assert!(!p.name.is_empty() && !p.description.is_empty());
        }
        assert!(CATALOG.iter().any(|p| p.core), "no core plugin");
    }

    #[test]
    fn absent_choice_means_everything() {
        let set = PluginSet::resolve(None);
        for p in CATALOG {
            assert!(set.contains(p.id));
        }
    }

    #[test]
    fn allow_list_keeps_core_implicitly() {
        let set = PluginSet::resolve(Some(&PluginChoice::Enabled(vec!["mealplan".into()])));
        assert!(set.contains("core"), "core is always on");
        assert!(set.contains("mealplan"));
        assert!(!set.contains("fitness"));
    }

    #[test]
    fn deny_list_cannot_disable_core() {
        let set = PluginSet::resolve(Some(&PluginChoice::Disabled(vec![
            "core".into(),
            "email".into(),
        ])));
        assert!(set.contains("core"), "core survives a deny-list");
        assert!(!set.contains("email"));
        assert!(set.contains("mealplan"));
    }

    #[test]
    fn unknown_ids_are_ignored_not_fatal() {
        let set = PluginSet::resolve(Some(&PluginChoice::Enabled(vec![
            "mealplan".into(),
            "not-a-plugin".into(),
        ])));
        assert!(set.contains("mealplan"));
        assert!(!set.contains("not-a-plugin"));
    }

    #[test]
    fn choice_serde_round_trips() {
        let c = PluginChoice::Disabled(vec!["email".into()]);
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<PluginChoice>(&s).unwrap(), c);
    }
}
