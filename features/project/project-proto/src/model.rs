//! `ProjectInfo` — canonical project entity.
//!
//! Source of truth: markdown vault page (`Projects/<slug>.md`)
//! with YAML frontmatter. The struct doubles as an
//! `architect::Entity`, so under the `server` feature the
//! SeaORM Model + `ProjectInfoRepoStorage<C>` are emitted —
//! the same schema mounts as a row without rewriting any
//! fields. See `feedback_architect_entity_default`.
//!
//! Field-level `#[serde(...)]` attributes drive frontmatter
//! shape (camelCase, skip-empty); they're stripped from the
//! synthetic `Create`/`Update` payloads by the architect
//! derive (it filters serde out of `forward_attrs`).

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype so architect can store it as a JSON
/// column. `Vec<T>` can't impl `From<Vec<T>> for sea_orm::Value`
/// directly (orphan rule); the `JsonField` derive emits the
/// four `sea_orm::Value` / `TryGetable` / `ValueType` /
/// `Nullable` impls via a `serde_json` round-trip.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Tags(pub Vec<String>);

impl From<Vec<String>> for Tags {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl Tags {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// t[impl project.definition.single] — the one definition. Not testable
// by any run, as `docs/spec/scenario-album.md` says: what violates it is
// a *second* struct appearing, so the marker sits on the first
/// A project with nothing said about it yet.
///
/// Hand-written rather than derived, because two fields have defaults
/// that are not their type's: `status` and `priority` have canonical
/// values, and `progress_percent` uses `-1` as "no tracked tasks yet"
/// rather than "0% done".
///
/// It exists so adding a field to [`ProjectInfo`] does not mean editing
/// every struct literal in the workspace. It meant seven, across the
/// server, the CLI, the UI and three test suites, the last time.
impl Default for ProjectInfo {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            path: String::new(),
            title: String::new(),
            status: "active".into(),
            priority: "normal".into(),
            project_type: String::new(),
            lead: String::new(),
            tags: Tags::default(),
            parts: crate::parts::Parts::default(),
            capabilities: crate::parts::Capabilities::default(),
            deliverables: crate::parts::Deliverables::default(),
            parent_id: None,
            same_as: None,
            target_date: None,
            progress_percent: default_progress(),
            details: String::new(),
            client_id: None,
            billable_default: false,
            currency: String::new(),
            default_rate_cents: 0,
            estimated_seconds: 0,
            agent_profile: String::new(),
            verify_command: String::new(),
            color: String::new(),
            image: String::new(),
            archived: false,
            states: None,
            date_created: None,
            date_modified: None,
        }
    }
}

/// One project. Lives as `Projects/<slug>.md` in the vault.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "projects", repo)]
pub struct ProjectInfo {
    /// Stable identity. Generated on first write; never
    /// re-derived from the path. Downstream features (timer,
    /// finance) reference projects by this UUID, so renaming
    /// the markdown file doesn't orphan their rows.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Vault-relative path. Not serialized into frontmatter
    /// (it'd duplicate the on-disk position). Under the
    /// `server` feature this becomes a DB column too, so
    /// reverse lookups (find project by path) are queryable.
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(filterable, sortable, fulltext)]
    pub title: String,

    /// One of [`Status`] as a stringly-typed slug; we accept
    /// any value so backends can add finer states without a
    /// schema bump.
    #[serde(default = "default_status")]
    #[architect(filterable, sortable)]
    pub status: String,

    /// Priority slug (`p0`..`p4`, `urgent`, `high`, `normal`,
    /// `low`, `lowest`). Defaults to `normal`. Used by the
    /// agent-dispatch cron to map onto the AgentTask priority
    /// when a project is dispatched wholesale.
    #[serde(default = "default_priority")]
    #[architect(filterable, sortable)]
    pub priority: String,

    /// Project type / template — `code` | `general` (default) |
    /// `personal`. Drives the overview layout: code projects lead with
    /// issues & PRs, personal projects hide the repo, general is the
    /// neutral default. Free-form so more types slot in without a
    /// schema bump; empty is treated as `general`.
    ///
    /// **Superseded by [`Self::capabilities`]**, and read-only from
    /// here on. `project.capability.closed` wants a closed vocabulary
    /// and this is a free string, so a page carrying only this parses
    /// into `capabilities` and saving emits `capabilities` instead. The
    /// field stays because every page in every vault has one and a
    /// vault we do not host cannot be migrated in place.
    #[serde(
        default,
        rename = "projectType",
        skip_serializing_if = "String::is_empty"
    )]
    #[architect(filterable)]
    pub project_type: String,

    /// Project lead / responsible party. Free-text (often a
    /// `[[User Name]]` wikilink). Multiple leads → join with
    /// `, ` in the frontmatter.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub lead: String,

    /// Tags. `project` is conventionally one of them, but
    /// not required — the scanner uses `type: project` OR
    /// `tags: [..., project]` as the discriminator.
    #[serde(skip_serializing_if = "Tags::is_empty", default)]
    #[architect(json)]
    pub tags: Tags,

    // t[impl project.nesting.uniform] — one entity, and the only thing a
    // subproject has that its parent may not is this field set. Nothing
    // downstream branches on depth, because there is nothing to branch on
    // t[impl project.nesting.explicit] — parentage is this declared link.
    // No directory name is consulted anywhere, and a child's page sits
    // wherever any other project's page sits
    /// Parent project. `None` for top-level projects;
    /// `Some(uuid)` for subprojects (e.g. `Fitness` /
    /// `Nutrition` / `Sleep Tracking` parented under
    /// `Health`). One level of nesting is what the existing
    /// UIs surface today; deeper trees are allowed but
    /// renderers may flatten beyond depth 1.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "parentId")]
    #[architect(filterable)]
    pub parent_id: Option<Uuid>,

    /// Federation pointer — `Some("@tombrooksmusic/png-worship-collective-album")`
    /// means this row is a *reference* to the canonical
    /// project owned by another org (e.g. a collaboration
    /// where one org leads and others participate). The
    /// resolver follows the link for full details.
    /// `None` for locally-owned projects.
    ///
    /// Mirror the same value in the body as a `[[@org/slug]]`
    /// wikilink so the page reads correctly in vanilla
    /// Obsidian. See the federated-platform design
    /// § federated wiki resolution for the `@org/slug`
    /// syntax.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "sameAs")]
    #[architect(filterable)]
    pub same_as: Option<String>,

    // ── Roadmap (Linear-style) ──────────────────────────────
    /// Target completion date (YYYY-MM-DD). `None` = open-ended.
    /// This + `progress_percent` are what make a Project a
    /// Linear-style roadmap initiative — no separate entity
    /// needed, the org-level Project already carried status,
    /// lead, and task/milestone rollup.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "targetDate"
    )]
    #[architect(filterable, sortable)]
    pub target_date: Option<NaiveDate>,

    /// Completion percentage, `0..=100`, or `-1` when undefined
    /// (no tracked tasks yet). Set by the rollup job from the
    /// ratio of done to total linked tasks. Stored signed so the
    /// `-1` sentinel survives.
    #[serde(default = "default_progress", rename = "progressPercent")]
    #[architect(filterable, sortable)]
    pub progress_percent: i16,

    /// Free-text description / body. Same convention as
    /// `task::TaskInfo::details`: everything after the
    /// frontmatter close fence lives here. Persisted in the
    /// DB column for full-text search; the markdown file
    /// remains the authoring surface.
    #[serde(skip)]
    #[architect(fulltext)]
    pub details: String,

    // ── Billing ─────────────────────────────────────────────
    /// Billable client (UUID) — points at a row in the
    /// `timer_clients` DB table. `None` for internal /
    /// non-billable projects (nullable column under SeaORM).
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "clientId")]
    #[architect(filterable)]
    pub client_id: Option<Uuid>,

    /// `true` if work on this project defaults to billable.
    /// Individual work sessions can still override.
    #[serde(default, rename = "billableDefault")]
    #[architect(filterable)]
    pub billable_default: bool,

    /// ISO 4217 currency code. Empty = non-billable or use
    /// org default. Mixing currencies within one project is
    /// forbidden — open a separate project.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    #[architect(filterable)]
    pub currency: String,

    /// Default hourly rate in cents. `0` = no project-level
    /// default; the rate cascade falls back to org / member
    /// rates. Snapshotted into `WorkSession.rate_cents` on
    /// close so retroactively changing the project rate
    /// doesn't re-bill old work.
    #[serde(default, rename = "defaultRateCents")]
    pub default_rate_cents: i64,

    /// Estimated total time in seconds. Drives "X of Y hours"
    /// indicators in the timer UI. `0` = no estimate.
    #[serde(default, rename = "estimatedSeconds")]
    pub estimated_seconds: i64,

    // ── Agent dispatch ──────────────────────────────────────
    /// Default agent profile for tasks dispatched under this
    /// project. Empty = inherit from the task note.
    #[serde(
        skip_serializing_if = "String::is_empty",
        default,
        rename = "agentProfile"
    )]
    pub agent_profile: String,

    /// Shell command whose **exit code is the verdict** on whether
    /// an agent's work on this project is done — `cargo check -p x`,
    /// `pnpm test`. Empty = inherit from the parent project; see
    /// [`crate::verify::resolve`].
    ///
    /// This exists so most tickets carry no verify command of their
    /// own. A ticket may still override it, and a ticket that
    /// resolves to nothing cannot be marked ready for an agent —
    /// otherwise "done" is an opinion rather than a fact.
    #[serde(
        skip_serializing_if = "String::is_empty",
        default,
        rename = "verifyCommand"
    )]
    pub verify_command: String,

    // ── UI ──────────────────────────────────────────────────
    /// Hex `#RRGGBB`. Empty = UI auto-picks from title hash.
    /// Used by the kanban + timer reports for column / pill
    /// colours.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub color: String,

    /// Optional cover image URL shown as a 16:9 banner on the
    /// project card. Empty = the card paints an accent-gradient
    /// placeholder instead.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub image: String,

    /// `false` while active. `true` once the project is
    /// closed out — kept on disk for historical timesheet
    /// integrity; new work sessions against an archived
    /// project are refused by the timer service.
    #[serde(default)]
    #[architect(filterable)]
    pub archived: bool,

    // ── Parts and capabilities ──────────────────────────────
    /// The project's named divisions — `project.part.unit`.
    ///
    /// A song, a scene, an episode. Costs nothing on disk beyond this
    /// list: no directory, no marker, no page. See
    /// [`crate::parts`] on why each carries an id from creation.
    #[serde(skip_serializing_if = "crate::parts::Parts::is_empty", default)]
    #[architect(json)]
    pub parts: crate::parts::Parts,

    /// What this project does — `project.capability.multiple`.
    ///
    /// A *set*, drawn from a closed vocabulary. Supersedes
    /// [`Self::project_type`]: a page carrying only the old field parses
    /// into this one, and saving writes this one. Never both.
    #[serde(skip_serializing_if = "crate::parts::Capabilities::is_empty", default)]
    #[architect(json)]
    pub capabilities: crate::parts::Capabilities,

    /// What this project produces for someone else —
    /// `project.deliverable.kind`.
    ///
    /// Declarations, not files: "per-song audio" is one entry however
    /// many songs there are. See [`crate::parts::Deliverable`].
    #[serde(skip_serializing_if = "crate::parts::Deliverables::is_empty", default)]
    #[architect(json)]
    pub deliverables: crate::parts::Deliverables,

    // ── State registry ──────────────────────────────────────
    /// Optional per-project state registry: custom status names
    /// bound to canonical [`crate::states::StateGroup`]s. `None`
    /// = use [`crate::states::default_states`] (the canonical
    /// open / in-progress / waiting / done / cancelled set).
    /// Task statuses stay free strings — this only adds the
    /// group *meaning* that rollups / burndown / kanban consume
    /// via [`crate::states::resolve_state_group`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(json)]
    pub states: Option<crate::states::StatesConfig>,

    // ── Timestamps ──────────────────────────────────────────
    /// Frontmatter aliases: `dateCreated` / `dateModified`
    /// (kept for back-compat with TaskNotes-style notes).
    /// Optional in the wire format — fresh projects might
    /// not have either set yet; the DB stores them nullable.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,
}

fn default_status() -> String {
    Status::Active.as_str().to_string()
}

fn default_priority() -> String {
    "normal".to_string()
}

/// `-1` = "no tracked tasks yet, progress undefined".
fn default_progress() -> i16 {
    -1
}

/// Built-in status values. Parsing accepts any string; these
/// are the recognized canonical forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Default for newly-created projects.
    Active,
    /// Hand-off complete, awaiting client sign-off, etc.
    OnHold,
    /// Finished + invoiced.
    Done,
    /// Cancelled without delivery.
    Cancelled,
    /// Dormant — nobody has touched it and nobody decided to stop.
    ///
    /// Distinct from [`Self::OnHold`], which is a *decision* to pause
    /// with an intent to resume, and from [`Self::Done`] /
    /// [`Self::Cancelled`], which are outcomes. Stale is the absence
    /// of any of those: an imported archive, a finished session nobody
    /// closed out, a project that quietly stopped.
    ///
    /// Not terminal ([`Self::is_closed`] stays false) — a stale
    /// project is one decision away from being any of the others. It
    /// exists so the default list can show what is genuinely current
    /// without pretending the rest were cancelled.
    Stale,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::OnHold => "on_hold",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Stale => "stale",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "active" | "open" | "in_progress" => Self::Active,
            "on_hold" | "on-hold" | "paused" | "waiting" => Self::OnHold,
            "done" | "complete" | "completed" | "shipped" => Self::Done,
            "cancelled" | "canceled" | "abandoned" => Self::Cancelled,
            "stale" | "dormant" | "archived" | "inactive" => Self::Stale,
            _ => return None,
        })
    }

    /// `true` once the project no longer accepts new work.
    #[must_use]
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    /// Should this project appear in the default project list?
    ///
    /// `Active` and `OnHold` only. A paused project is still current —
    /// you decided to pause it and you mean to come back. Stale is
    /// not: nobody decided anything, which is exactly why showing it
    /// alongside live work makes the list useless.
    ///
    /// Separate from [`Self::is_closed`] on purpose. "Is this
    /// finished?" and "should I be looking at this?" are different
    /// questions, and a stale project answers no to the first and no
    /// to the second without being either.
    #[must_use]
    pub fn is_current(self) -> bool {
        matches!(self, Self::Active | Self::OnHold)
    }
}

#[cfg(test)]
mod stale_status_tests {
    use super::Status;

    #[test]
    fn stale_is_neither_closed_nor_current() {
        // The whole reason it exists: a project nobody decided about
        // is not finished, and not something to look at either.
        assert!(!Status::Stale.is_closed());
        assert!(!Status::Stale.is_current());
    }

    #[test]
    fn on_hold_is_current_because_pausing_was_a_decision() {
        assert!(Status::OnHold.is_current());
        assert!(!Status::OnHold.is_closed());
    }

    #[test]
    fn outcomes_are_closed_and_not_current() {
        for s in [Status::Done, Status::Cancelled] {
            assert!(s.is_closed());
            assert!(!s.is_current());
        }
    }

    #[test]
    fn stale_round_trips_and_accepts_the_words_people_actually_write() {
        assert_eq!(Status::from_str("stale"), Some(Status::Stale));
        for alias in ["dormant", "archived", "inactive", "STALE"] {
            assert_eq!(Status::from_str(alias), Some(Status::Stale), "{alias}");
        }
        assert_eq!(
            Status::from_str(Status::Stale.as_str()),
            Some(Status::Stale)
        );
    }
}
