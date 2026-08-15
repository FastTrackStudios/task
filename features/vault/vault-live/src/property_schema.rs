//! Phase 6.5 — typed property schema for Knowledge `Page` frontmatter.
//!
//! Built on the observation that `Page.frontmatter_json: String`
//! (top-level, untyped) blocks faithful modelling of real-world
//! task / project / area pages. Specifically, the `TaskNotes` plugin
//! (`~/Development/research/tasknotes/src/types.ts`) needs:
//!
//! - nested structs in lists (`blockedBy: [{uid, reltype}]`),
//! - typed dates for Bases comparisons,
//! - status with attached metadata (color / icon / isCompleted /
//!   autoArchive),
//! - a concurrent-safe ordering primitive (`LexoRank`).
//!
//! This module gives us a registry of `KindSchema`s — one per
//! `kind:` frontmatter convention. The Bases executor + the
//! server-side `FrontmatterIndex` consult the registry to coerce
//! values and decompose container types.
//!
//! ## Storage model (hybrid)
//!
//! Built-in schemas (`task`, `project`, `area`, `person`, `daily`)
//! ship hardcoded in this module's [`builtins`] function. The
//! server merges in user-defined schemas read from `kind: schema`
//! Pages in `vault://org` at boot — Phase 6.5a wires the merge
//! path but the page-borne schemas can be empty.
//!
//! ## Strictness (coerce-or-shadow)
//!
//! When a value's type doesn't match the declared `PropertyType`,
//! [`coerce_value`] tries to convert. If the conversion fails the
//! original is preserved in a sibling `__shadow__<key>` JSON
//! entry on the page's frontmatter, and a warning is surfaced.
//! This mirrors Obsidian's behavior: don't reject the write,
//! just flag it.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// What kind of value a property holds. Drives coercion + the
/// `FrontmatterIndex` decomposition + Bases evaluator typing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PropertyType {
    /// Single-line text.
    Text,
    /// Free-form multi-line text (YAML block scalar). Renders as a
    /// textarea in the properties pane.
    Multitext,
    Number,
    Checkbox,
    /// `YYYY-MM-DD`.
    Date,
    /// ISO-8601 with timezone.
    Datetime,
    /// Special: union with inline `#tags` from block content.
    Tags,
    /// Special: feeds the basename resolver (Phase 4) — these are
    /// alternate names a page answers to.
    Aliases,
    /// Single `[[wikilink]]` reference to another page.
    Link,
    /// List of wikilinks. Covers `up`, `projects`, etc.
    LinkList,
    /// Enum with per-option metadata — `status`, `priority`.
    EnumWithMetadata {
        options: Vec<EnumOption>,
    },
    /// Nested structured object. The named fields are themselves
    /// `PropertyDef`s. List`<Struct>` is expressed via the outer
    /// container.
    Struct {
        fields: Vec<PropertyDef>,
    },
    /// List of structs — `blockedBy`, `timeEntries`, `reminders`.
    StructList {
        fields: Vec<PropertyDef>,
    },
    /// Computed at read time. Not written to disk.
    /// `formula` is an opaque string the evaluator can interpret;
    /// Phase 6.5a doesn't evaluate them — it just registers the
    /// field so consumers can know it's computed.
    Computed {
        formula: String,
    },
    /// CRDT-safe ordering primitive. Stored as a string; sorted
    /// lexicographically. Concurrent inserts produce mergeable
    /// keys (insert-between, no swap fight).
    LexoRank,
    /// Escape hatch — opaque JSON. The properties pane will show
    /// raw YAML for these.
    Json,
}

/// One option of an [`PropertyType::EnumWithMetadata`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnumOption {
    /// On-disk value — e.g. `"todo"`, `"in_progress"`, `"done"`.
    pub value: String,
    /// Display label — e.g. `"To do"`, `"In progress"`.
    pub label: String,
    /// Tailwind / architect-ui color token. None = theme default.
    pub color: Option<String>,
    /// Lucide icon name. Optional.
    pub icon: Option<String>,
    /// Display order (and kanban column order).
    pub order: u32,
    /// Semantic flag: drops a `completedDate` write + enables the
    /// auto-archive flow when set.
    pub is_completed: bool,
    /// Auto-archive delay in days. `None` = never auto-archive.
    pub auto_archive_delay_days: Option<u32>,
    /// Sort weight (priority). Higher = more important.
    pub weight: Option<i32>,
}

/// Declaration of one property on a `KindSchema`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyDef {
    /// Frontmatter key on disk.
    pub key: String,
    pub ty: PropertyType,
    /// Whether the page must carry this field. The validator
    /// surfaces a warning on a missing required field but never
    /// rejects the write.
    #[serde(default)]
    pub required: bool,
    /// Default to use when the field is missing.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Human label for the properties pane.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Lucide icon name for the properties pane row.
    #[serde(default)]
    pub icon: Option<String>,
}

/// All declared properties for one `kind:` value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KindSchema {
    pub kind: String,
    /// Simple single-parent inheritance. The merge appends parent
    /// fields the child doesn't override.
    #[serde(default)]
    pub extends: Option<String>,
    pub properties: Vec<PropertyDef>,
}

/// Renames for core fields. Lets the user call our `status`
/// whatever they want without breaking the schema — matches
/// `TaskNotes`' `FieldMapping` pattern.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldRenames {
    pub map: IndexMap<String, String>,
}

impl FieldRenames {
    pub fn resolve<'a>(&'a self, canonical: &'a str) -> &'a str {
        self.map.get(canonical).map_or(canonical, String::as_str)
    }
}

/// Registry holding the resolved (parent-merged) schemas keyed by
/// `kind`. Built from [`builtins`] + any user-supplied schemas.
#[derive(Clone, Debug, Default)]
pub struct PropertySchemaRegistry {
    schemas: IndexMap<String, KindSchema>,
    renames: FieldRenames,
}

impl PropertySchemaRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with built-in schemas pre-loaded.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        for s in builtins() {
            r.insert(s);
        }
        r
    }

    /// Insert (or replace) a schema. Inheritance is resolved on
    /// lookup, not on insert — so order-of-insert doesn't matter.
    pub fn insert(&mut self, schema: KindSchema) {
        self.schemas.insert(schema.kind.clone(), schema);
    }

    /// Lookup the resolved schema for `kind`. Resolved = parent
    /// fields are appended (child overrides win by key).
    #[must_use]
    pub fn get(&self, kind: &str) -> Option<KindSchema> {
        let base = self.schemas.get(kind)?;
        if base.extends.is_none() {
            return Some(base.clone());
        }
        let mut resolved = base.clone();
        let mut visited = std::collections::HashSet::new();
        visited.insert(resolved.kind.clone());
        let mut cursor = resolved.extends.clone();
        while let Some(parent_kind) = cursor {
            if !visited.insert(parent_kind.clone()) {
                // Cycle — bail without panicking.
                break;
            }
            let Some(parent) = self.schemas.get(&parent_kind) else {
                // extends unknown parent — bail.
                break;
            };
            for p in &parent.properties {
                if !resolved.properties.iter().any(|own| own.key == p.key) {
                    resolved.properties.push(p.clone());
                }
            }
            cursor = parent.extends.clone();
        }
        // Drop the `extends` link in the returned shape — already
        // flattened.
        resolved.extends = None;
        Some(resolved)
    }

    #[must_use]
    pub fn kinds(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    #[must_use]
    pub fn renames(&self) -> &FieldRenames {
        &self.renames
    }

    pub fn set_renames(&mut self, renames: FieldRenames) {
        self.renames = renames;
    }

    /// Resolve the property def for `kind` + `key` (after applying
    /// renames). Returns the def the executor / indexer should use.
    #[must_use]
    pub fn lookup(&self, kind: &str, key: &str) -> Option<PropertyDef> {
        let resolved = self.get(kind)?;
        let canonical = self.renames.resolve(key);
        resolved
            .properties
            .into_iter()
            .find(|p| p.key == canonical || p.key == key)
    }
}

/// Outcome of coercing a value to a declared `PropertyType`.
#[derive(Debug, Clone, PartialEq)]
pub enum CoerceOutcome {
    /// Value matched (or was successfully coerced). Carries the
    /// normalized value.
    Ok(serde_json::Value),
    /// Value didn't match and couldn't be coerced. Carries the
    /// original (callers should write it to a `__shadow__<key>`
    /// sibling) plus a human-readable reason.
    Shadow {
        original: serde_json::Value,
        reason: String,
    },
}

/// Best-effort coerce. Per design decision: never reject — fall
/// back to `Shadow` so the original survives. Phase 6.5a covers
/// the common cases; nested-struct coercion is intentionally
/// permissive (object stays as `Json` if its shape doesn't match).
#[must_use]
pub fn coerce_value(ty: &PropertyType, value: serde_json::Value) -> CoerceOutcome {
    use serde_json::Value as V;
    match (ty, &value) {
        // Text-ish types.
        (PropertyType::Text | PropertyType::Multitext, V::String(_)) => CoerceOutcome::Ok(value),
        (PropertyType::Text | PropertyType::Multitext, V::Null) => {
            CoerceOutcome::Ok(V::String(String::new()))
        }
        (PropertyType::Text | PropertyType::Multitext, V::Number(n)) => {
            CoerceOutcome::Ok(V::String(n.to_string()))
        }
        (PropertyType::Text | PropertyType::Multitext, V::Bool(b)) => {
            CoerceOutcome::Ok(V::String(b.to_string()))
        }

        // Number.
        (PropertyType::Number, V::Number(_)) => CoerceOutcome::Ok(value),
        (PropertyType::Number, V::String(s)) => match s.parse::<f64>() {
            Ok(f) => match serde_json::Number::from_f64(f) {
                Some(n) => CoerceOutcome::Ok(V::Number(n)),
                None => CoerceOutcome::Shadow {
                    original: value.clone(),
                    reason: "number not representable as f64".into(),
                },
            },
            Err(_) => CoerceOutcome::Shadow {
                original: value.clone(),
                reason: format!("`{s}` is not a number"),
            },
        },

        (PropertyType::Checkbox, V::Bool(_)) => CoerceOutcome::Ok(value),
        (PropertyType::Checkbox, V::String(s)) => match s.as_str() {
            "true" | "yes" | "1" => CoerceOutcome::Ok(V::Bool(true)),
            "false" | "no" | "0" | "" => CoerceOutcome::Ok(V::Bool(false)),
            _ => CoerceOutcome::Shadow {
                original: value.clone(),
                reason: format!("`{s}` is not a bool"),
            },
        },

        // Date: accept either YYYY-MM-DD strings or ISO datetimes
        // (truncating to date). We don't bring chrono into the
        // proto crate — keep the validation as a regex-y check.
        (PropertyType::Date, V::String(s)) => {
            if looks_like_date(s) {
                CoerceOutcome::Ok(value.clone())
            } else if looks_like_datetime(s) {
                // Take the first 10 chars (YYYY-MM-DD prefix).
                CoerceOutcome::Ok(V::String(s.chars().take(10).collect()))
            } else {
                CoerceOutcome::Shadow {
                    original: value.clone(),
                    reason: format!("`{s}` is not YYYY-MM-DD"),
                }
            }
        }
        (PropertyType::Datetime, V::String(s)) => {
            if looks_like_datetime(s) || looks_like_date(s) {
                CoerceOutcome::Ok(value.clone())
            } else {
                CoerceOutcome::Shadow {
                    original: value.clone(),
                    reason: format!("`{s}` is not an ISO datetime"),
                }
            }
        }

        // Tags / Aliases / LinkList / Multitext-of-strings — all
        // accept arrays of strings. A single string becomes a
        // one-element list.
        (PropertyType::Tags | PropertyType::Aliases | PropertyType::LinkList, V::Array(_)) => {
            CoerceOutcome::Ok(value)
        }
        (PropertyType::Tags | PropertyType::Aliases | PropertyType::LinkList, V::String(s)) => {
            CoerceOutcome::Ok(V::Array(vec![V::String(s.clone())]))
        }
        (PropertyType::Tags | PropertyType::Aliases | PropertyType::LinkList, V::Null) => {
            CoerceOutcome::Ok(V::Array(Vec::new()))
        }

        // Link — accept a string (assume wikilink).
        (PropertyType::Link, V::String(_) | V::Null) => CoerceOutcome::Ok(value),

        // EnumWithMetadata — value must be one of the declared
        // options. Otherwise shadow.
        (PropertyType::EnumWithMetadata { options }, V::String(s)) => {
            if options.iter().any(|o| &o.value == s) {
                CoerceOutcome::Ok(value.clone())
            } else {
                CoerceOutcome::Shadow {
                    original: value.clone(),
                    reason: format!("`{s}` not in enum"),
                }
            }
        }

        // Struct / StructList / Computed / LexoRank / Json — accept
        // as-is (Phase 6.5a is permissive). Tighter validation in 6.6.
        (
            PropertyType::Struct { .. }
            | PropertyType::StructList { .. }
            | PropertyType::Json
            | PropertyType::LexoRank,
            _,
        ) => CoerceOutcome::Ok(value),
        // Computed fields shouldn't ever land here (they're not
        // written to disk), but be permissive if a stale value is
        // sitting in frontmatter.
        (PropertyType::Computed { .. }, _) => CoerceOutcome::Ok(value),

        // Everything else: shadow with a clear reason.
        (ty, v) => CoerceOutcome::Shadow {
            original: v.clone(),
            reason: format!("value shape doesn't match {ty:?}"),
        },
    }
}

fn looks_like_date(s: &str) -> bool {
    s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

fn looks_like_datetime(s: &str) -> bool {
    // Permissive: starts with YYYY-MM-DD and has a T or space.
    s.len() >= 10
        && looks_like_date(&s[..10])
        && (s.len() == 10 || matches!(s.as_bytes().get(10), Some(b'T' | b' ')))
}

// ── Built-in schemas ─────────────────────────────────────────────────

/// The hardcoded built-in schemas. Modelled on `TaskNotes` (per
/// `~/Development/research/tasknotes/src/types.ts:445-711`) and on
/// the top-frequency frontmatter keys observed in The Observatory
/// (8885 .md, 81% with frontmatter; top keys include `up`,
/// `Aliases`, `previous`/`next`, `tags`, `title`, `status`,
/// `priority`, `due`).
#[must_use]
pub fn builtins() -> Vec<KindSchema> {
    vec![
        node_schema(),
        person_schema(),
        area_schema(),
        project_schema(),
        task_schema(),
        daily_schema(),
    ]
}

/// Build an [`EnumOption`] with a color + icon + order (weight = order).
fn enum_opt(value: &str, label: &str, color: &str, icon: &str, order: u32) -> EnumOption {
    EnumOption {
        value: value.into(),
        label: label.into(),
        color: Some(color.into()),
        icon: Some(icon.into()),
        order,
        is_completed: false,
        auto_archive_delay_days: None,
        weight: Some(order as i32),
    }
}

/// The epistemic / knowledge-graph node properties — `confidence`,
/// `visibility`, `maturity` — shared by every page via the `node` base
/// schema. These mark how sure a note is (facts vs opinions), whether it
/// publishes, and how mature it is. See `plans/knowledge-primitives.md`.
#[must_use]
pub fn epistemic_properties() -> Vec<PropertyDef> {
    vec![
        PropertyDef {
            key: "confidence".into(),
            ty: PropertyType::EnumWithMetadata {
                options: vec![
                    enum_opt("speculative", "Speculative", "muted", "circle-help", 0),
                    enum_opt("unlikely", "Unlikely", "muted", "circle-dashed", 1),
                    enum_opt("possible", "Possible", "amber", "circle-dot", 2),
                    enum_opt("likely", "Likely", "sky", "circle-check", 3),
                    enum_opt("certain", "Certain", "emerald", "badge-check", 4),
                ],
            },
            required: false,
            default: None,
            display_name: Some("Confidence".into()),
            icon: Some("gauge".into()),
        },
        PropertyDef {
            key: "visibility".into(),
            ty: PropertyType::EnumWithMetadata {
                options: vec![
                    enum_opt("private", "Private", "muted", "lock", 0),
                    enum_opt("unlisted", "Unlisted", "amber", "eye-off", 1),
                    enum_opt("public", "Public", "emerald", "globe", 2),
                ],
            },
            required: false,
            // Opt-in: private by default, so nothing publishes by accident.
            default: Some(serde_json::Value::String("private".into())),
            display_name: Some("Visibility".into()),
            icon: Some("eye".into()),
        },
        PropertyDef {
            key: "maturity".into(),
            ty: PropertyType::EnumWithMetadata {
                options: vec![
                    enum_opt("seedling", "Seedling", "amber", "sprout", 0),
                    enum_opt("budding", "Budding", "lime", "leaf", 1),
                    enum_opt("evergreen", "Evergreen", "emerald", "trees", 2),
                ],
            },
            required: false,
            default: None,
            display_name: Some("Maturity".into()),
            icon: Some("sprout".into()),
        },
    ]
}

/// The `node` base schema every page kind extends — carries the
/// epistemic properties so any vault note or wiki page can declare its
/// confidence / visibility / maturity.
fn node_schema() -> KindSchema {
    KindSchema {
        kind: "node".into(),
        extends: None,
        properties: epistemic_properties(),
    }
}

fn person_schema() -> KindSchema {
    KindSchema {
        kind: "person".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "auth_user_id".into(),
                ty: PropertyType::Text,
                required: false,
                default: None,
                display_name: Some("Auth user id".into()),
                icon: Some("user".into()),
            },
            PropertyDef {
                key: "aliases".into(),
                ty: PropertyType::Aliases,
                required: false,
                default: None,
                display_name: Some("Aliases".into()),
                icon: None,
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
        ],
    }
}

fn area_schema() -> KindSchema {
    KindSchema {
        kind: "area".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "up".into(),
                ty: PropertyType::LinkList,
                required: false,
                default: None,
                display_name: Some("Up".into()),
                icon: Some("arrow-up".into()),
            },
            PropertyDef {
                key: "icon".into(),
                ty: PropertyType::Text,
                required: false,
                default: None,
                display_name: Some("Icon".into()),
                icon: None,
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
        ],
    }
}

fn project_schema() -> KindSchema {
    KindSchema {
        kind: "project".into(),
        extends: Some("area".into()),
        properties: vec![
            PropertyDef {
                key: "state".into(),
                ty: PropertyType::EnumWithMetadata {
                    options: vec![
                        EnumOption {
                            value: "planning".into(),
                            label: "Planning".into(),
                            color: Some("muted".into()),
                            icon: Some("clipboard-list".into()),
                            order: 0,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "active".into(),
                            label: "Active".into(),
                            color: Some("primary".into()),
                            icon: Some("play".into()),
                            order: 1,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "on_hold".into(),
                            label: "On hold".into(),
                            color: Some("warning".into()),
                            icon: Some("pause".into()),
                            order: 2,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "done".into(),
                            label: "Done".into(),
                            color: Some("success".into()),
                            icon: Some("check".into()),
                            order: 3,
                            is_completed: true,
                            auto_archive_delay_days: Some(30),
                            weight: None,
                        },
                    ],
                },
                required: false,
                default: Some(serde_json::Value::String("planning".into())),
                display_name: Some("State".into()),
                icon: Some("clipboard-list".into()),
            },
            PropertyDef {
                key: "start".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Start".into()),
                icon: Some("calendar".into()),
            },
            PropertyDef {
                key: "due".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Due".into()),
                icon: Some("calendar".into()),
            },
        ],
    }
}

fn task_schema() -> KindSchema {
    KindSchema {
        kind: "task".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "title".into(),
                ty: PropertyType::Text,
                required: true,
                default: None,
                display_name: Some("Title".into()),
                icon: None,
            },
            PropertyDef {
                key: "status".into(),
                ty: PropertyType::EnumWithMetadata {
                    options: vec![
                        EnumOption {
                            value: "todo".into(),
                            label: "To do".into(),
                            color: Some("muted".into()),
                            icon: Some("circle".into()),
                            order: 0,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "in_progress".into(),
                            label: "In progress".into(),
                            color: Some("primary".into()),
                            icon: Some("circle-dot".into()),
                            order: 1,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "blocked".into(),
                            label: "Blocked".into(),
                            color: Some("warning".into()),
                            icon: Some("octagon-x".into()),
                            order: 2,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "done".into(),
                            label: "Done".into(),
                            color: Some("success".into()),
                            icon: Some("circle-check".into()),
                            order: 3,
                            is_completed: true,
                            auto_archive_delay_days: Some(7),
                            weight: None,
                        },
                    ],
                },
                required: false,
                default: Some(serde_json::Value::String("todo".into())),
                display_name: Some("Status".into()),
                icon: Some("circle".into()),
            },
            PropertyDef {
                key: "priority".into(),
                ty: PropertyType::EnumWithMetadata {
                    options: vec![
                        EnumOption {
                            value: "low".into(),
                            label: "Low".into(),
                            color: Some("muted".into()),
                            icon: None,
                            order: 0,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(0),
                        },
                        EnumOption {
                            value: "normal".into(),
                            label: "Normal".into(),
                            color: Some("primary".into()),
                            icon: None,
                            order: 1,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(10),
                        },
                        EnumOption {
                            value: "high".into(),
                            label: "High".into(),
                            color: Some("warning".into()),
                            icon: Some("chevron-up".into()),
                            order: 2,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(20),
                        },
                        EnumOption {
                            value: "urgent".into(),
                            label: "Urgent".into(),
                            color: Some("danger".into()),
                            icon: Some("flame".into()),
                            order: 3,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(30),
                        },
                    ],
                },
                required: false,
                default: Some(serde_json::Value::String("normal".into())),
                display_name: Some("Priority".into()),
                icon: None,
            },
            PropertyDef {
                key: "due".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Due".into()),
                icon: Some("calendar".into()),
            },
            PropertyDef {
                key: "scheduled".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Scheduled".into()),
                icon: Some("calendar-clock".into()),
            },
            PropertyDef {
                key: "completed_date".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Completed".into()),
                icon: None,
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
            PropertyDef {
                key: "contexts".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Contexts".into()),
                icon: Some("at-sign".into()),
            },
            PropertyDef {
                key: "projects".into(),
                ty: PropertyType::LinkList,
                required: false,
                default: None,
                display_name: Some("Projects".into()),
                icon: Some("folder".into()),
            },
            PropertyDef {
                key: "blocked_by".into(),
                ty: PropertyType::StructList {
                    fields: vec![
                        PropertyDef {
                            key: "uid".into(),
                            ty: PropertyType::Link,
                            required: true,
                            default: None,
                            display_name: Some("Blocker".into()),
                            icon: None,
                        },
                        PropertyDef {
                            key: "reltype".into(),
                            ty: PropertyType::Text,
                            required: false,
                            default: Some(serde_json::Value::String("FINISHTOSTART".into())),
                            display_name: Some("Relation".into()),
                            icon: None,
                        },
                        PropertyDef {
                            key: "gap".into(),
                            ty: PropertyType::Text,
                            required: false,
                            default: None,
                            display_name: Some("Gap (ISO 8601)".into()),
                            icon: None,
                        },
                    ],
                },
                required: false,
                default: None,
                display_name: Some("Blocked by".into()),
                icon: Some("ban".into()),
            },
            PropertyDef {
                key: "time_estimate".into(),
                ty: PropertyType::Number,
                required: false,
                default: None,
                display_name: Some("Estimate (min)".into()),
                icon: Some("hourglass".into()),
            },
            PropertyDef {
                key: "sort_order".into(),
                ty: PropertyType::LexoRank,
                required: false,
                default: None,
                display_name: None,
                icon: None,
            },
        ],
    }
}

fn daily_schema() -> KindSchema {
    KindSchema {
        kind: "daily".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "previous".into(),
                ty: PropertyType::Link,
                required: false,
                default: None,
                display_name: Some("Previous".into()),
                icon: Some("chevron-left".into()),
            },
            PropertyDef {
                key: "next".into(),
                ty: PropertyType::Link,
                required: false,
                default: None,
                display_name: Some("Next".into()),
                icon: Some("chevron-right".into()),
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
        ],
    }
}

#[cfg(test)]
#[allow(clippy::match_wildcard_for_single_variants)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_all_kinds() {
        let r = PropertySchemaRegistry::with_builtins();
        for kind in &["node", "task", "project", "area", "person", "daily"] {
            assert!(r.get(kind).is_some(), "missing {kind} schema");
        }
    }

    #[test]
    fn page_kinds_inherit_epistemic_node_properties() {
        let r = PropertySchemaRegistry::with_builtins();
        // Direct (person → node) and transitive (project → area → node).
        for kind in &["person", "project", "daily", "task"] {
            let schema = r.get(kind).unwrap_or_else(|| panic!("{kind} schema"));
            let keys: Vec<&str> = schema.properties.iter().map(|p| p.key.as_str()).collect();
            for prop in &["confidence", "visibility", "maturity"] {
                assert!(keys.contains(prop), "{kind} should inherit {prop}");
            }
        }
    }

    #[test]
    fn project_inherits_area_fields() {
        let r = PropertySchemaRegistry::with_builtins();
        let project = r.get("project").expect("project schema");
        // project has `state`, `start`, `due`; area contributes `up`,
        // `icon`, `tags`. All should be present in the resolved view.
        let keys: Vec<&str> = project.properties.iter().map(|p| p.key.as_str()).collect();
        assert!(keys.contains(&"state"));
        assert!(keys.contains(&"up"), "should inherit `up` from area");
        assert!(keys.contains(&"icon"), "should inherit `icon` from area");
        assert!(keys.contains(&"tags"), "should inherit `tags` from area");
    }

    #[test]
    fn coerce_string_number_round_trip() {
        match coerce_value(&PropertyType::Number, serde_json::json!("42")) {
            CoerceOutcome::Ok(serde_json::Value::Number(n)) => {
                assert_eq!(n.as_f64(), Some(42.0));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn coerce_bad_number_shadows() {
        match coerce_value(&PropertyType::Number, serde_json::json!("not-a-number")) {
            CoerceOutcome::Shadow { original, .. } => {
                assert_eq!(original, serde_json::json!("not-a-number"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn coerce_iso_datetime_to_date_truncates() {
        match coerce_value(
            &PropertyType::Date,
            serde_json::json!("2026-05-14T10:30:00Z"),
        ) {
            CoerceOutcome::Ok(serde_json::Value::String(s)) => {
                assert_eq!(s, "2026-05-14");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn coerce_enum_unknown_shadows() {
        let task = PropertySchemaRegistry::with_builtins()
            .get("task")
            .expect("task schema");
        let status = task
            .properties
            .iter()
            .find(|p| p.key == "status")
            .expect("status field");
        match coerce_value(&status.ty, serde_json::json!("nonsense")) {
            CoerceOutcome::Shadow { .. } => {}
            other => panic!("expected shadow, got {other:?}"),
        }
        match coerce_value(&status.ty, serde_json::json!("in_progress")) {
            CoerceOutcome::Ok(_) => {}
            other => panic!("expected ok, got {other:?}"),
        }
    }

    #[test]
    fn coerce_single_string_to_tag_list() {
        match coerce_value(&PropertyType::Tags, serde_json::json!("solo")) {
            CoerceOutcome::Ok(serde_json::Value::Array(items)) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0], serde_json::json!("solo"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn task_has_blocked_by_struct_list() {
        let task = PropertySchemaRegistry::with_builtins()
            .get("task")
            .expect("task schema");
        let blocked_by = task
            .properties
            .iter()
            .find(|p| p.key == "blocked_by")
            .expect("blocked_by field");
        assert!(matches!(blocked_by.ty, PropertyType::StructList { .. }));
    }

    #[test]
    fn renames_resolve() {
        let mut r = PropertySchemaRegistry::with_builtins();
        let mut renames = FieldRenames::default();
        renames.map.insert("status".into(), "task-status".into());
        r.set_renames(renames);
        assert_eq!(r.renames().resolve("status"), "task-status");
    }
}
