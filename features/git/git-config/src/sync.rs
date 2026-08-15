//! Per-field conflict resolution between a local `TaskInfo` and
//! the forge `Issue` it's linked to via [`crate::IssueLink`].
//!
//! `plans/issue-tracker-integration.md` documents a per-field
//! source-of-truth split (superseding the old coarse "forge wins for
//! open/closed, full stop" rule that lost every Task-only field the
//! moment the two sides diverged):
//!
//! | Field                                            | Wins  |
//! |--------------------------------------------------|-------|
//! | `title`, `body`, `state`                         | Forge |
//! | `assignees-on-forge`, `labels-on-forge`, `milestone` | Forge |
//! | `priority`, `cycle`, `project`, `estimate`       | Task  |
//! | `agent-attribution`, `task-only-labels`          | Task  |
//!
//! This module is the **pure, testable** core of that model. The
//! field-agnostic primitives ([`resolve_field`] / [`resolve_conflict`])
//! take per-field provenance and return verdicts; [`reconcile_synced`]
//! layers on the concrete scalar projection (`title` / `body` /
//! open-closed `state`) the live `task issue sync` path drives,
//! deriving provenance by value-diffing each side against the
//! last-converged [`crate::IssueSnapshot`]. Pushing task-won
//! forge-owned edits back to the forge, and extending the projection
//! to labels / assignees / milestone, are follow-up subtasks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Which side last modified a field. The provenance tracked
/// alongside every syncable field so resolution is per-field, not
/// whole-record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldSource {
    /// The local Task (vault markdown + DB row).
    Task,
    /// The forge issue (GitHub / Forgejo).
    Forge,
}

/// The set of syncable fields shared (in part) between a
/// `TaskInfo` and a forge `Issue`. Each field maps to a fixed
/// *owner* under the documented source-of-truth split; the owner
/// wins on a genuine divergence regardless of which side is
/// "newer", which is what makes the model predictable rather than
/// pure last-writer-wins.
///
/// Ownership is a static property of the field, so [`Field::owner`]
/// is `const`. The per-record provenance timestamps only matter as
/// a tie-break when the two sides disagree about a *forge-owned*
/// field but the forge value is stale relative to a local edit —
/// see [`resolve_field`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    // ── Forge-owned ──────────────────────────────────────────
    /// Issue title (`TaskInfo::title` ↔ `Issue::title`).
    Title,
    /// Issue body (`TaskInfo::details` ↔ `Issue::body`).
    Body,
    /// Open/closed state (`TaskInfo::status` projection ↔
    /// `Issue::state`).
    State,
    /// Assignees as the forge models them (`Issue::assignees`).
    Assignees,
    /// Labels that exist on the forge (`Issue::labels`).
    ForgeLabels,
    /// Milestone (`Issue::milestone`).
    Milestone,

    // ── Task-owned ───────────────────────────────────────────
    /// Priority (`TaskInfo::priority`) — the forge doesn't model
    /// it.
    Priority,
    /// Active cycle / sprint (`WorkflowAttrs::cycle`).
    Cycle,
    /// Linear-sense project (`TaskInfo::project_id`).
    Project,
    /// Work estimate (`WorkflowAttrs::estimate`).
    Estimate,
    /// Agent attribution (`WorkflowAttrs::assignees`, the
    /// `AgentRef` claim) — distinct from forge `Assignees`.
    AgentAttribution,
    /// Labels that only exist locally (tags not present on the
    /// forge).
    TaskOnlyLabels,
}

impl Field {
    /// Every syncable field, in declaration order. Handy for
    /// driving a full-record reconcile and for exhaustiveness
    /// tests.
    pub const ALL: [Field; 12] = [
        Field::Title,
        Field::Body,
        Field::State,
        Field::Assignees,
        Field::ForgeLabels,
        Field::Milestone,
        Field::Priority,
        Field::Cycle,
        Field::Project,
        Field::Estimate,
        Field::AgentAttribution,
        Field::TaskOnlyLabels,
    ];

    /// The fixed owner for this field under the documented split.
    /// This is the source that wins on a genuine conflict.
    #[must_use]
    pub const fn owner(self) -> FieldSource {
        match self {
            // Forge owns the substrate-native fields.
            Field::Title
            | Field::Body
            | Field::State
            | Field::Assignees
            | Field::ForgeLabels
            | Field::Milestone => FieldSource::Forge,
            // Task owns the workflow fields the forge can't model.
            Field::Priority
            | Field::Cycle
            | Field::Project
            | Field::Estimate
            | Field::AgentAttribution
            | Field::TaskOnlyLabels => FieldSource::Task,
        }
    }

    /// `true` when the *forge* is the source of truth for this
    /// field.
    #[must_use]
    pub const fn forge_wins(self) -> bool {
        matches!(self.owner(), FieldSource::Forge)
    }

    /// `true` when the *Task* is the source of truth for this
    /// field.
    #[must_use]
    pub const fn task_wins(self) -> bool {
        matches!(self.owner(), FieldSource::Task)
    }
}

/// Per-field provenance: who last touched each side, and when.
///
/// `task_modified` / `forge_modified` are the timestamps the two
/// sides last wrote the field. `None` means "never observed to
/// change on that side" — treated as infinitely old for the
/// tie-break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldProvenance {
    /// When the Task side last modified this field.
    pub task_modified: Option<DateTime<Utc>>,
    /// When the forge side last modified this field.
    pub forge_modified: Option<DateTime<Utc>>,
}

impl FieldProvenance {
    /// Both sides untouched.
    #[must_use]
    pub const fn never() -> Self {
        Self {
            task_modified: None,
            forge_modified: None,
        }
    }

    /// Only the Task side moved.
    #[must_use]
    pub const fn task_only(at: DateTime<Utc>) -> Self {
        Self {
            task_modified: Some(at),
            forge_modified: None,
        }
    }

    /// Only the forge side moved.
    #[must_use]
    pub const fn forge_only(at: DateTime<Utc>) -> Self {
        Self {
            task_modified: None,
            forge_modified: Some(at),
        }
    }

    /// Both sides moved.
    #[must_use]
    pub const fn both(task_at: DateTime<Utc>, forge_at: DateTime<Utc>) -> Self {
        Self {
            task_modified: Some(task_at),
            forge_modified: Some(forge_at),
        }
    }

    /// `true` when *both* sides modified the field since the last
    /// reconcile — the only case where ownership has to break a
    /// genuine tie.
    #[must_use]
    pub const fn both_changed(&self) -> bool {
        self.task_modified.is_some() && self.forge_modified.is_some()
    }
}

/// The decision for one field: which side to copy from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    /// The field this verdict applies to.
    pub field: Field,
    /// Which side wins — copy its value into the other.
    pub winner: FieldSource,
}

impl Resolution {
    /// `true` when the winning side differs from `side` — i.e. a
    /// write back to `side` is needed to converge.
    #[must_use]
    pub fn writes_to(&self, side: FieldSource) -> bool {
        self.winner != side
    }
}

/// Resolve a single field given its per-field provenance.
///
/// The rule, in order:
///
/// 1. If only one side changed, that side wins — there's no
///    conflict to arbitrate, so a stale-but-unchanged value never
///    clobbers a real edit on the other side.
/// 2. If *both* sides changed (a genuine conflict), the field's
///    static [owner](Field::owner) wins. This is the documented
///    source-of-truth split: forge for substrate-native fields,
///    Task for workflow fields.
/// 3. If neither side changed, the owner wins by convention (a
///    no-op in practice — both values already agree — but keeps
///    the function total and deterministic).
///
/// Step 1 is what makes this better than the current coarse
/// "forge always wins for state": a local-only edit to a
/// forge-owned field still loses to the forge *only if the forge
/// also moved*. When the forge hasn't touched it, the local edit
/// stands until the next forge change.
#[must_use]
pub fn resolve_field(field: Field, prov: &FieldProvenance) -> Resolution {
    let winner = match (prov.task_modified, prov.forge_modified) {
        // Only Task moved.
        (Some(_), None) => FieldSource::Task,
        // Only forge moved.
        (None, Some(_)) => FieldSource::Forge,
        // Both moved — genuine conflict, owner arbitrates.
        // Neither moved — owner by convention.
        (Some(_), Some(_)) | (None, None) => field.owner(),
    };
    Resolution { field, winner }
}

/// Resolve every field of a `TaskInfo` ↔ `Issue` pair given
/// per-field provenance, returning one [`Resolution`] per field.
///
/// `provenance` supplies the timestamps for each field; any field
/// absent from the map is treated as [`FieldProvenance::never`]
/// (neither side changed → owner wins, a no-op). Pure: it reads
/// the closure and returns verdicts; the caller applies them.
#[must_use]
pub fn resolve_conflict(provenance: impl Fn(Field) -> FieldProvenance) -> Vec<Resolution> {
    Field::ALL
        .iter()
        .map(|&f| resolve_field(f, &provenance(f)))
        .collect()
}
// ── Live-sync integration ────────────────────────────────────────────
//
// The resolver above is field-agnostic. To drive it from the live
// `task issue sync` path we need (a) a concrete projection of the
// forge-owned fields that map cleanly onto a `TaskInfo`, and (b) a
// way to turn "did this side change since the last reconcile?" into
// the [`FieldProvenance`] the resolver consumes. Per-field timestamps
// would be one source of that signal; a cheaper one — used here — is a
// **value diff against the last-converged baseline** (a snapshot the
// caller persists per [`crate::IssueLink`]). `resolve_field` only
// inspects whether each side's timestamp is `Some`/`None`, so a
// changed/unchanged bit is all the provenance it actually needs.
//
// Scope note: only the scalar forge-owned fields that map onto a plain
// `TaskInfo` field today — `title`, `body`, open/closed `state` — are
// projected here. Labels / assignees / milestone are forge-owned too
// but need richer mapping (the task-only-label split, a forge-assignee
// field, milestone-by-title); they're tracked as follow-up subtasks of
// the per-field-resolution issue and intentionally left out of the
// projection rather than mapped half-way.

/// The scalar forge-owned projection synced bidirectionally between a
/// `TaskInfo` and its linked forge `Issue`: title, body, and the
/// open/closed state bit (`TaskInfo::status == "done"` ⇄
/// `Issue::state == Closed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedFields {
    pub title: String,
    pub body: String,
    /// `true` ⇒ closed/done.
    pub closed: bool,
}

/// A sentinel "this field moved" timestamp. `resolve_field` only reads
/// the `Some`/`None` shape of a [`FieldProvenance`], never the instant,
/// so any fixed value works — the Unix epoch keeps it obvious in a
/// debugger that the time is a placeholder, not a real edit time.
fn moved() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid timestamp")
}

fn provenance(task_changed: bool, forge_changed: bool) -> FieldProvenance {
    FieldProvenance {
        task_modified: task_changed.then(moved),
        forge_modified: forge_changed.then(moved),
    }
}

/// Reconcile the scalar forge-owned projection of one linked
/// `TaskInfo` ↔ `Issue` pair, returning the **converged value for each
/// field** (what the Task side should hold after the merge).
///
/// `base_*` are the values each side held at the last reconcile (the
/// persisted snapshot); `task` / `forge` are the current values. A
/// field counts as changed on a side when it differs from that side's
/// baseline. The per-field [`resolve_field`] verdict then picks the
/// winner: a forge-only edit lands locally, a task-only edit to a
/// forge-owned field stands until the forge next moves it, and a
/// genuine both-sides conflict goes to the field's owner (forge, for
/// all three of these).
#[must_use]
pub fn reconcile_synced(
    base_task: &SyncedFields,
    base_forge: &SyncedFields,
    task: &SyncedFields,
    forge: &SyncedFields,
) -> SyncedFields {
    let resolve = |field: Field, task_changed: bool, forge_changed: bool| {
        resolve_field(field, &provenance(task_changed, forge_changed)).winner
    };

    let title = match resolve(
        Field::Title,
        task.title != base_task.title,
        forge.title != base_forge.title,
    ) {
        FieldSource::Forge => forge.title.clone(),
        FieldSource::Task => task.title.clone(),
    };
    let body = match resolve(
        Field::Body,
        task.body != base_task.body,
        forge.body != base_forge.body,
    ) {
        FieldSource::Forge => forge.body.clone(),
        FieldSource::Task => task.body.clone(),
    };
    let closed = match resolve(
        Field::State,
        task.closed != base_task.closed,
        forge.closed != base_forge.closed,
    ) {
        FieldSource::Forge => forge.closed,
        FieldSource::Task => task.closed,
    };

    SyncedFields {
        title,
        body,
        closed,
    }
}

/// The forge-side write needed for the forge to match the reconciled
/// (`merged`) values — the inverse of the pull. A field is `Some` only
/// when it diverges from what the forge currently holds (i.e. the Task
/// side won a forge-owned field and the local edit must be pushed
/// back); `None` means already in agreement, leave it alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForgeUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    /// `Some(true)` ⇒ close on the forge, `Some(false)` ⇒ reopen.
    pub closed: Option<bool>,
}

impl ForgeUpdate {
    /// `true` when nothing needs writing to the forge.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.body.is_none() && self.closed.is_none()
    }
}

/// Compute the forge-side write that converges the forge with `merged`.
/// Pairs with [`reconcile_synced`]: that resolves the merged values and
/// applies the forge→Task half; this names the Task→forge half so the
/// caller can issue an `update_issue`.
#[must_use]
pub fn forge_update(forge: &SyncedFields, merged: &SyncedFields) -> ForgeUpdate {
    ForgeUpdate {
        title: (merged.title != forge.title).then(|| merged.title.clone()),
        body: (merged.body != forge.body).then(|| merged.body.clone()),
        closed: (merged.closed != forge.closed).then_some(merged.closed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    // ── Static ownership: the documented source-of-truth split ──

    #[test]
    fn forge_owns_substrate_fields() {
        for f in [
            Field::Title,
            Field::Body,
            Field::State,
            Field::Assignees,
            Field::ForgeLabels,
            Field::Milestone,
        ] {
            assert_eq!(f.owner(), FieldSource::Forge, "{f:?} should be forge-owned");
            assert!(f.forge_wins(), "{f:?} forge_wins");
            assert!(!f.task_wins(), "{f:?} not task_wins");
        }
    }

    #[test]
    fn task_owns_workflow_fields() {
        for f in [
            Field::Priority,
            Field::Cycle,
            Field::Project,
            Field::Estimate,
            Field::AgentAttribution,
            Field::TaskOnlyLabels,
        ] {
            assert_eq!(f.owner(), FieldSource::Task, "{f:?} should be task-owned");
            assert!(f.task_wins(), "{f:?} task_wins");
            assert!(!f.forge_wins(), "{f:?} not forge_wins");
        }
    }

    #[test]
    fn all_covers_every_field_exactly_once() {
        // Six forge-owned + six task-owned = twelve, no dupes.
        assert_eq!(Field::ALL.len(), 12);
        let forge = Field::ALL.iter().filter(|f| f.forge_wins()).count();
        let task = Field::ALL.iter().filter(|f| f.task_wins()).count();
        assert_eq!(forge, 6);
        assert_eq!(task, 6);
    }

    // ── resolve_field: one-sided change always wins ────────────

    #[test]
    fn task_only_change_wins_even_on_forge_owned_field() {
        // Local edited the title; forge untouched. The local edit
        // stands — this is the improvement over "forge always
        // wins for its fields".
        let r = resolve_field(Field::Title, &FieldProvenance::task_only(ts(100)));
        assert_eq!(r.winner, FieldSource::Task);
    }

    #[test]
    fn forge_only_change_wins_even_on_task_owned_field() {
        // Forge somehow moved a task-owned field (e.g. a label we
        // also use locally); Task untouched → take the forge value.
        let r = resolve_field(Field::Priority, &FieldProvenance::forge_only(ts(100)));
        assert_eq!(r.winner, FieldSource::Forge);
    }

    // ── resolve_field: genuine conflict → owner arbitrates ─────

    #[test]
    fn both_changed_forge_owned_field_forge_wins_regardless_of_time() {
        // Forge timestamp OLDER than Task's, but the field is
        // forge-owned → forge still wins. Ownership beats recency
        // for a genuine conflict on an owned field.
        let prov = FieldProvenance::both(ts(200), ts(100));
        let r = resolve_field(Field::State, &prov);
        assert_eq!(r.winner, FieldSource::Forge);

        // And when forge is newer, same verdict.
        let prov = FieldProvenance::both(ts(100), ts(200));
        let r = resolve_field(Field::State, &prov);
        assert_eq!(r.winner, FieldSource::Forge);
    }

    #[test]
    fn both_changed_task_owned_field_task_wins_regardless_of_time() {
        let prov = FieldProvenance::both(ts(100), ts(200));
        let r = resolve_field(Field::Priority, &prov);
        assert_eq!(r.winner, FieldSource::Task);

        let prov = FieldProvenance::both(ts(200), ts(100));
        let r = resolve_field(Field::Priority, &prov);
        assert_eq!(r.winner, FieldSource::Task);
    }

    #[test]
    fn neither_changed_falls_back_to_owner() {
        let r = resolve_field(Field::Body, &FieldProvenance::never());
        assert_eq!(r.winner, FieldSource::Forge);
        let r = resolve_field(Field::Estimate, &FieldProvenance::never());
        assert_eq!(r.winner, FieldSource::Task);
    }

    // ── The acceptance-criteria scenario ───────────────────────

    #[test]
    fn human_closes_on_forge_while_agent_edits_priority_in_task() {
        // From plans/issue-tracker-integration.md acceptance
        // criteria: "human closes on GitHub while agent edits
        // priority in Task → both wins land in their respective
        // sources."
        let resolutions = resolve_conflict(|f| match f {
            // Human closed the issue on the forge.
            Field::State => FieldProvenance::forge_only(ts(500)),
            // Agent bumped priority locally.
            Field::Priority => FieldProvenance::task_only(ts(600)),
            _ => FieldProvenance::never(),
        });

        let state = resolutions
            .iter()
            .find(|r| r.field == Field::State)
            .unwrap();
        let prio = resolutions
            .iter()
            .find(|r| r.field == Field::Priority)
            .unwrap();

        // Forge's close wins → write the closed state back to Task.
        assert_eq!(state.winner, FieldSource::Forge);
        assert!(state.writes_to(FieldSource::Task));
        // Task's priority wins → (if it weren't a no-op on the forge)
        // it stays Task-side; the forge never gets a priority.
        assert_eq!(prio.winner, FieldSource::Task);
        assert!(prio.writes_to(FieldSource::Forge));
    }

    #[test]
    fn resolve_conflict_returns_one_verdict_per_field() {
        let rs = resolve_conflict(|_| FieldProvenance::never());
        assert_eq!(rs.len(), Field::ALL.len());
        for f in Field::ALL {
            assert!(rs.iter().any(|r| r.field == f), "missing verdict for {f:?}");
        }
    }

    #[test]
    fn writes_to_only_when_winner_differs() {
        let r = Resolution {
            field: Field::Title,
            winner: FieldSource::Forge,
        };
        assert!(r.writes_to(FieldSource::Task));
        assert!(!r.writes_to(FieldSource::Forge));
    }

    #[test]
    fn provenance_both_changed_predicate() {
        assert!(FieldProvenance::both(ts(1), ts(2)).both_changed());
        assert!(!FieldProvenance::task_only(ts(1)).both_changed());
        assert!(!FieldProvenance::forge_only(ts(1)).both_changed());
        assert!(!FieldProvenance::never().both_changed());
    }

    // ── reconcile_synced: the live-sync projection ──────────────────

    fn sf(title: &str, body: &str, closed: bool) -> SyncedFields {
        SyncedFields {
            title: title.into(),
            body: body.into(),
            closed,
        }
    }

    #[test]
    fn forge_only_edit_lands_locally() {
        // Baseline agrees; forge renamed the issue, Task untouched.
        let base = sf("old", "b", false);
        let task = sf("old", "b", false);
        let forge = sf("new", "b", false);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        assert_eq!(merged.title, "new");
    }

    #[test]
    fn forge_close_lands_locally() {
        let base = sf("t", "b", false);
        let task = sf("t", "b", false);
        let forge = sf("t", "b", true);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        assert!(merged.closed);
    }

    #[test]
    fn task_only_edit_to_forge_owned_field_is_kept() {
        // Task renamed locally; forge unchanged. A forge-owned field,
        // but with no forge movement the local edit stands.
        let base = sf("old", "b", false);
        let task = sf("local", "b", false);
        let forge = sf("old", "b", false);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        assert_eq!(merged.title, "local");
    }

    #[test]
    fn both_changed_forge_owned_field_forge_wins() {
        // Genuine conflict on a forge-owned field → owner (forge) wins.
        let base = sf("old", "b", false);
        let task = sf("task-rename", "b", false);
        let forge = sf("forge-rename", "b", false);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        assert_eq!(merged.title, "forge-rename");
    }

    #[test]
    fn unchanged_pair_is_a_noop() {
        let base = sf("t", "b", false);
        let merged = reconcile_synced(&base, &base, &base, &base);
        assert_eq!(merged, base);
    }

    #[test]
    fn first_sync_seeds_baseline_from_task_so_forge_wins_divergence() {
        // Models the CLI's first-reconcile seeding: with no snapshot,
        // the baseline is the *task* projection on both sides, so a
        // forge that disagrees on a forge-owned field registers as a
        // forge-side change and wins — preserving "forge wins for
        // state" on first contact.
        let task = sf("t", "b", false);
        let forge = sf("t", "b", true); // forge has it closed
        let merged = reconcile_synced(&task, &task, &task, &forge);
        assert!(merged.closed);
    }

    #[test]
    fn forge_update_pushes_a_task_won_field() {
        // Task renamed locally, forge unmoved → the merge kept the
        // local title, and the forge now needs that title pushed.
        let base = sf("old", "b", false);
        let task = sf("local", "b", false);
        let forge = sf("old", "b", false);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        let fu = forge_update(&forge, &merged);
        assert_eq!(fu.title.as_deref(), Some("local"));
        assert_eq!(fu.body, None);
        assert_eq!(fu.closed, None);
        assert!(!fu.is_empty());
    }

    #[test]
    fn forge_update_is_empty_when_forge_won() {
        // Forge renamed; the merge took the forge value, so there's
        // nothing to push back.
        let base = sf("old", "b", false);
        let task = sf("old", "b", false);
        let forge = sf("forge-rename", "b", false);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        let fu = forge_update(&forge, &merged);
        assert!(fu.is_empty());
    }

    #[test]
    fn forge_update_pushes_a_local_reopen() {
        // Local reopened a done task while the forge still has it
        // closed; the state push reopens it on the forge.
        let base = sf("t", "b", true);
        let task = sf("t", "b", false); // reopened locally
        let forge = sf("t", "b", true);
        let merged = reconcile_synced(&base, &base, &task, &forge);
        let fu = forge_update(&forge, &merged);
        assert_eq!(fu.closed, Some(false));
    }

    #[test]
    fn acceptance_forge_close_while_task_edits_an_unsynced_field() {
        // The #127 scenario, projected: the forge closes the issue
        // while the agent's local edit is to a Task-owned field
        // (priority) that isn't part of SyncedFields at all. The state
        // close lands; nothing in the projection disturbs the local
        // priority, which the caller never feeds in.
        let base = sf("t", "b", false);
        let task = sf("t", "b", false); // title/body/state untouched locally
        let forge = sf("t", "b", true); // forge closed it
        let merged = reconcile_synced(&base, &base, &task, &forge);
        assert!(merged.closed);
        assert_eq!(merged.title, "t");
        assert_eq!(merged.body, "b");
    }
}
