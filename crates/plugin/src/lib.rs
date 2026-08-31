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
//! Design: the plugin system — ids, catalog, and `PluginSet`.

use serde::{Deserialize, Serialize};

/// Identity and metadata for one plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginInfo {
    /// Stable machine id — what `org.toml` stores and surfaces key on.
    /// Lowercase ASCII + `-`.
    ///
    /// Renaming one is a data change, not a text change: the old
    /// spelling is on disk in orgs configured before the rename. If it
    /// has to happen, add the old id to [`RENAMED`] in the same commit
    /// so those orgs keep the plugin they chose.
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
///
/// # Core is what apps are built on
///
/// The line is not "how big" or "how central to us" — it is whether an
/// *app* needs it to exist. Scheduling, contacts, the wiki and files
/// are all reached by apps that are not them: a meal plan schedules, a
/// finance record names a contact, everything stores markdown, and
/// every one of them keeps its data in Task's file management. Something
/// an app depends on cannot be a thing an org turns off, so those are
/// core.
///
/// What is left is genuinely a domain of its own — scripture, email,
/// finance, fitness, inventory, meal planning. Turning one off removes
/// its screens and its services and nothing else notices.
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
        core: true,
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
        id: "git",
        name: "Git",
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
        core: true,
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
        core: true,
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
        core: true,
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
    let id = canonical(id);
    CATALOG.iter().find(|p| p.id == id)
}

/// Ids this catalog used to use, and what they are now.
///
/// A plugin id is not just a constant in this file — it is written in
/// people's `org.toml`, on disk, in orgs that were configured before
/// the rename. Dropping the old spelling would not error; [`resolve`]
/// would log "unknown plugin" and quietly return a set without it, and
/// the symptom would be tabs that stopped appearing for no visible
/// reason. So old names keep working.
///
/// [`resolve`]: PluginSet::resolve
const RENAMED: &[(&str, &str)] = &[
    // Was "Forges" (GitHub / Forgejo). The proto crate behind it was
    // always `git-proto`; the id now says the same thing.
    ("forge", "git"),
];

/// The current id for a possibly-old one.
#[must_use]
pub fn canonical(id: &str) -> &str {
    RENAMED
        .iter()
        .find_map(|(was, now)| (*was == id).then_some(*now))
        .unwrap_or(id)
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
                    .filter(|p| p.core || ids.iter().any(|i| canonical(i) == p.id))
                    .map(|p| p.id)
                    .collect()
            }
            Some(PluginChoice::Disabled(ids)) => {
                warn_unknown(ids);
                CATALOG
                    .iter()
                    .filter(|p| p.core || !ids.iter().any(|i| canonical(i) == p.id))
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

    /// An org configured before the rename still gets what it chose.
    /// The failure this guards is silent: an unknown id is warned about
    /// and dropped, so the symptom would be tabs quietly missing.
    #[test]
    fn an_org_that_asked_for_forge_gets_git() {
        let set = PluginSet::resolve(Some(&PluginChoice::Enabled(vec!["forge".into()])));
        assert!(set.contains("git"), "the old spelling still turns it on");
    }

    #[test]
    fn an_org_that_turned_forge_off_still_has_it_off() {
        let set = PluginSet::resolve(Some(&PluginChoice::Disabled(vec!["forge".into()])));
        assert!(!set.contains("git"), "the old spelling still turns it off");
        assert!(set.contains("email"), "and takes nothing else with it");
    }

    /// A renamed id is not "unknown" — it resolves, so it must not be
    /// warned about as a typo.
    #[test]
    fn an_old_name_is_found_not_unknown() {
        assert_eq!(find("forge").map(|p| p.id), Some("git"));
        assert_eq!(canonical("forge"), "git");
        assert_eq!(canonical("git"), "git", "a current id is left alone");
        assert_eq!(canonical("nonsense"), "nonsense");
    }

    /// Every rename must land somewhere real, or it silently does
    /// nothing — and the old id starts reading as a typo again.
    #[test]
    fn every_rename_points_at_a_plugin_that_exists() {
        for (was, now) in RENAMED {
            assert!(
                CATALOG.iter().any(|p| p.id == *now),
                "{was} renames to {now}, which is not in the catalog"
            );
            assert!(
                !CATALOG.iter().any(|p| p.id == *was),
                "{was} is both a live id and a renamed one"
            );
        }
    }
}
