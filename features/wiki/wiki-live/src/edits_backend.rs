//! The `Edits` service: how someone without Editor changes a wiki, and
//! how an Editor lands it.
//!
//! Three things meet here and the shape follows from keeping them
//! apart:
//!
//! - **The wiki** ([`WikiBackend`]) owns the pages. Every landing goes
//!   through its own `write_page`, so the sha guard, the change stream
//!   and the Editor check are the same ones a direct write meets.
//! - **The tracker** ([`Tracker`]) owns the request's status as an
//!   issue (`wiki.edit.tracked`). This crate cannot depend on the task
//!   feature — it has to stay a wiki — so the tracker is a trait the
//!   server implements over the org's tasks, and a test implements in
//!   memory.
//! - **The store** ([`crate::edits`]) owns what neither of those can:
//!   the change itself, the base the proposer saw, the claim.
//!
//! Every method records the wiki and the outcome on the current span
//! (`wiki.slug`, `wiki.edit.id`, `wiki.edit.outcome`); no page content
//! and no principal ever rides a field.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use architect_telemetry::wide;
use chrono::{DateTime, Utc};
use wiki_proto::WikiEvent;
use wiki_proto::config::{ProposerGate, WikiConfig};
use wiki_proto::error::WikiError;
use wiki_proto::log as ctypes;
use wiki_proto::service::edits::{
    EditRequest, EditStatus, Editors, Edits, NewEditRequest, PageChange, PageDiff,
};
use wiki_proto::service::{Catalog, Pages};

use crate::WikiBackend;
use crate::edits as store;
use crate::repo_source;

// The forge hook used to live here, when accepting a request on a
// repo-sourced wiki pushed it. Accepting now writes the working copy
// like any other wiki and pushing is the Registry's `push_changes`, so
// the hook moved beside the git code; these names stay for callers.
pub use crate::repo_source::{ForgeIdentity, Lander, NoForge};

// ────────────────────── Tracker ──────────────────────

/// The row the tracker opens for a request. The id is the request's:
/// one thing, two views (`wiki.edit.tracked`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIssue {
    pub id: uuid::Uuid,
    pub title: String,
    pub details: String,
    pub tags: Vec<String>,
}

/// The tag every Edit Request's row carries, beside `task` and
/// `wiki:<slug>`. What an issue view filters on.
pub const EDIT_REQUEST_TAG: &str = "edit-request";

/// The status a row is closed with when its request is accepted.
pub const ISSUE_DONE: &str = "done";
/// The status a row is closed with when its request is rejected.
pub const ISSUE_CANCELLED: &str = "cancelled";

/// Whether a tracker status means the row is closed. The tracker's
/// vocabulary is the task feature's; these are its closed words.
#[must_use]
pub fn issue_is_closed(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "done" | "completed" | "complete" | "cancelled" | "canceled"
    )
}

/// The org's issue tracker, as far as the Edit lane needs it.
pub trait Tracker: Send + Sync + 'static {
    /// Open a row with exactly this id. Returns the id.
    fn open_issue(&self, issue: NewIssue) -> Result<uuid::Uuid, String>;
    /// The row's current status word, or `None` when the row is gone.
    fn issue_status(&self, id: uuid::Uuid) -> Result<Option<String>, String>;
    /// Close the row with this status, appending `note` to its details.
    fn close_issue(&self, id: uuid::Uuid, status: &str, note: &str) -> Result<(), String>;
}

/// A [`Tracker`] in memory, for tests and for a backend with no org
/// tracker behind it.
#[derive(Default)]
pub struct MemoryTracker {
    rows: Mutex<Vec<(NewIssue, String)>>,
}

impl MemoryTracker {
    /// Every row, with its status.
    #[must_use]
    pub fn rows(&self) -> Vec<(NewIssue, String)> {
        self.rows.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Set a row's status from the issue side, the way a person closing
    /// the issue would.
    pub fn set_status(&self, id: uuid::Uuid, status: &str) {
        if let Ok(mut rows) = self.rows.lock() {
            if let Some(row) = rows.iter_mut().find(|(i, _)| i.id == id) {
                row.1 = status.to_owned();
            }
        }
    }
}

impl Tracker for MemoryTracker {
    fn open_issue(&self, issue: NewIssue) -> Result<uuid::Uuid, String> {
        let id = issue.id;
        self.rows
            .lock()
            .map_err(|_| "tracker lock".to_owned())?
            .push((issue, "open".to_owned()));
        Ok(id)
    }

    fn issue_status(&self, id: uuid::Uuid) -> Result<Option<String>, String> {
        Ok(self
            .rows
            .lock()
            .map_err(|_| "tracker lock".to_owned())?
            .iter()
            .find(|(i, _)| i.id == id)
            .map(|(_, s)| s.clone()))
    }

    fn close_issue(&self, id: uuid::Uuid, status: &str, note: &str) -> Result<(), String> {
        let mut rows = self.rows.lock().map_err(|_| "tracker lock".to_owned())?;
        let row = rows
            .iter_mut()
            .find(|(i, _)| i.id == id)
            .ok_or_else(|| format!("no issue {id}"))?;
        row.1 = status.to_owned();
        if !note.is_empty() {
            row.0.details.push_str("\n\n");
            row.0.details.push_str(note);
        }
        Ok(())
    }
}

// ────────────────────── Backend ──────────────────────

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// How long a claim stands before it expires on its own
/// (`wiki.edit.claim`).
pub const DEFAULT_CLAIM_TTL: Duration = Duration::from_secs(60 * 60);

/// The Edit lane over one org's wikis.
///
/// Dispatched inline rather than on a blocking thread: the gate records
/// the caller in a task-local on the async side, and a `spawn_blocking`
/// hop loses it — every proposer would read as nobody and every Editor
/// check would fail closed. The work here is small JSON and one page
/// write, so inline is the right shape anyway.
#[derive(Clone, architect::HasDispatcher)]
#[dispatch(architect::dispatch::CurrentThreadDispatcher)]
pub struct EditsBackend {
    wiki: WikiBackend,
    tracker: Arc<dyn Tracker>,
    claim_ttl: Duration,
    clock: Clock,
    /// Serialises state transitions: two Editors claiming at once must
    /// see one another, and a landing must not interleave with a
    /// revision.
    lock: Arc<Mutex<()>>,
}

impl EditsBackend {
    #[must_use]
    pub fn new(wiki: WikiBackend, tracker: Arc<dyn Tracker>) -> Self {
        Self {
            wiki,
            tracker,
            claim_ttl: DEFAULT_CLAIM_TTL,
            clock: Arc::new(Utc::now),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// How long a claim stands.
    #[must_use]
    pub fn with_claim_ttl(mut self, ttl: Duration) -> Self {
        self.claim_ttl = ttl;
        self
    }

    /// Replace the clock, so a test can let a claim expire without
    /// waiting for it.
    #[must_use]
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// The wiki backend this lane lands into.
    #[must_use]
    pub fn wiki(&self) -> &WikiBackend {
        &self.wiki
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The caller, or a refusal: the lane records who did what, and
    /// cannot record nobody.
    fn principal(&self) -> Result<String, WikiError> {
        self.wiki
            .calling_principal()
            .ok_or_else(|| refused("the Edit lane needs a signed-in caller".into()))
    }

    /// The caller as an Editor of this wiki, or a refusal naming why.
    fn require_editor(&self, wiki_id: &str) -> Result<(String, WikiConfig), WikiError> {
        let principal = self.principal()?;
        let config = self.wiki.config_of(wiki_id)?;
        if !config.is_editor(&principal) {
            return Err(refused(format!(
                "not an Editor of `{wiki_id}`; Editors are the only ones who land, decline or \
                 return a request"
            )));
        }
        Ok((principal, config))
    }

    /// Load a request and bring it up to date with the clock and the
    /// tracker: an expired claim reads back as none, and a row closed
    /// from the issue surface closes the request here
    /// (`wiki.edit.tracked`). Whatever changed is written back.
    fn load(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError> {
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = store::load(&root, id)?;
        if self.reconcile(&mut request) {
            store::save(&root, &request)?;
        }
        Ok(request)
    }

    fn reconcile(&self, request: &mut EditRequest) -> bool {
        let now = self.now();
        let mut changed = false;
        if !request.claimed_until.is_empty() {
            let expired = DateTime::parse_from_rfc3339(&request.claimed_until)
                .map(|until| until.with_timezone(&Utc) <= now)
                .unwrap_or(true);
            if expired {
                request.claimed_by.clear();
                request.claimed_until.clear();
                changed = true;
            }
        }
        if request.status.is_open() {
            match self.tracker.issue_status(request.id) {
                Ok(Some(status)) if issue_is_closed(&status) => {
                    request.status = EditStatus::Closed;
                    request.resolved_at = now.to_rfc3339();
                    if request.resolution.is_empty() {
                        request.resolution = format!("closed from the tracker ({status})");
                    }
                    request.claimed_by.clear();
                    request.claimed_until.clear();
                    changed = true;
                }
                Ok(_) => {}
                Err(_) => wide::set("wiki.edit.tracker", "unreachable"),
            }
        }
        changed
    }

    /// Whether this caller may see a held request: its proposer and
    /// the Editors, nobody else (`wiki.edit.gate`).
    fn may_see(&self, request: &EditRequest, config: &WikiConfig) -> bool {
        if !request.held {
            return true;
        }
        match self.wiki.calling_principal() {
            None => true,
            Some(p) => request.proposer == p || config.is_editor(&p),
        }
    }

    /// Whether another Editor's claim stands on this request.
    fn claimed_by_someone_else(&self, request: &EditRequest, principal: &str) -> bool {
        !request.claimed_by.is_empty() && request.claimed_by != principal
    }

    /// Each change against the page as it is now (`wiki.edit.reviewable`,
    /// `wiki.edit.rebase`).
    fn diffs(&self, wiki_id: &str, changes: &[PageChange]) -> Result<Vec<PageDiff>, WikiError> {
        changes
            .iter()
            .map(|change| {
                let (current, current_sha) = match self.wiki.read_page(wiki_id, &change.path) {
                    Ok(doc) => (doc.markdown, doc.sha256),
                    Err(WikiError::NotFound(_)) => (String::new(), String::new()),
                    Err(e) => return Err(e),
                };
                let stale = current_sha != change.base_sha256;
                let (applies, merged) = if change.delete {
                    // Deleting a page somebody has since changed is a
                    // decision, not a merge.
                    (!stale, String::new())
                } else if !stale {
                    (true, change.markdown.clone())
                } else {
                    match diffy::merge(&change.base_markdown, &current, &change.markdown) {
                        Ok(merged) => (true, merged),
                        Err(_) => (false, String::new()),
                    }
                };
                Ok(PageDiff {
                    path: change.path.clone(),
                    current,
                    proposed: if change.delete {
                        String::new()
                    } else {
                        change.markdown.clone()
                    },
                    stale,
                    applies,
                    merged,
                })
            })
            .collect()
    }

    /// Land a request: every page's merge first, then every write, then
    /// the log entry that attributes it. Nothing is written when any
    /// page conflicts (`wiki.edit.rebase`).
    fn land(
        &self,
        wiki_id: &str,
        request: &mut EditRequest,
        editor: &str,
    ) -> Result<(), WikiError> {
        let diffs = self.diffs(wiki_id, &request.changes)?;
        let conflicts: Vec<&str> = diffs
            .iter()
            .filter(|d| !d.applies)
            .map(|d| d.path.as_str())
            .collect();
        if !conflicts.is_empty() {
            wide::set("wiki.edit.outcome", "conflict");
            return Err(WikiError::Conflict(format!(
                "{} changed since the request was made and the changes overlap: {}",
                if conflicts.len() == 1 {
                    "a page"
                } else {
                    "pages"
                },
                conflicts.join(", ")
            )));
        }
        let root = self.wiki.root_of(wiki_id)?;
        let now = self.now();

        // t[impl wiki.source.editable] — a repo-sourced wiki is no
        // different here: the accepted change lands in the working copy
        // (the live, collaboratively edited version of the repository's
        // pages) and nothing is pushed. The working copy reaches the
        // repository when an Editor pushes it — one branch, one commit,
        // one pull request — through the Registry's `push_changes`.
        for (change, diff) in request.changes.iter().zip(&diffs) {
            if change.delete {
                let abs = root.join(&change.path);
                match std::fs::remove_file(&abs) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(WikiError::Io(format!("{}: {e}", abs.display()))),
                }
                self.wiki.emit(
                    wiki_id,
                    WikiEvent::PageDeleted {
                        path: change.path.clone(),
                        at: now,
                    },
                );
            } else {
                let base = if diff.current.is_empty() {
                    String::new()
                } else {
                    crate::backend::sha256_hex(diff.current.as_bytes())
                };
                self.wiki
                    .write_page(wiki_id, &change.path, &diff.merged, &base)?;
            }
        }
        // The attribution `wiki.edit.reviewable` asks for: the landing
        // names the request, its proposer and the Editor who accepted.
        self.wiki.append_log(
            wiki_id,
            ctypes::LogEntry {
                id: uuid::Uuid::new_v4(),
                at: now,
                op: ctypes::LogOp::Review,
                title: format!("Edit Request accepted: {}", request.title),
                body: format!(
                    "Edit Request {} proposed by {}, accepted by {}.{}",
                    request.id,
                    request.proposer,
                    editor,
                    if request.summary.trim().is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", request.summary.trim())
                    }
                ),
                pages_touched: ctypes::WikilinkList(
                    request.changes.iter().map(|c| c.path.clone()).collect(),
                ),
            },
        )?;
        request.status = EditStatus::Accepted;
        request.resolved_by = editor.to_owned();
        request.resolved_at = now.to_rfc3339();
        request.resolution = "accepted".into();
        request.claimed_by.clear();
        request.claimed_until.clear();
        Ok(())
    }

    /// Settle any request still `Landing` from before accepting wrote
    /// the working copy — one pushed on its own branch by an older
    /// server — against what the repository now holds: a commit the
    /// base reaches becomes `Accepted` and closes its tracker row.
    /// Called after each sync; returns the ids that landed. Nothing
    /// enters `Landing` any more, so on a current deployment this is a
    /// no-op; the pending *push* is settled by the sync itself
    /// (`repo_source::sync` clears `RepoSource::pending`).
    pub fn reconcile_landings(&self, wiki_id: &str) -> Result<Vec<uuid::Uuid>, WikiError> {
        let config = self.wiki.config_of(wiki_id)?;
        let Some(source) = config.source.as_ref() else {
            return Ok(Vec::new());
        };
        let root = self.wiki.root_of(wiki_id)?;
        let Some(wikis_dir) = root.parent() else {
            return Ok(Vec::new());
        };
        let _guard = self.guard();
        let mut landed = Vec::new();
        for mut request in store::list(&root)? {
            if request.status != EditStatus::Landing {
                continue;
            }
            let Some(commit) = request.resolution.rsplit(' ').next().map(str::to_owned) else {
                continue;
            };
            if !repo_source::contains_commit(wikis_dir, wiki_id, source, &commit) {
                continue;
            }
            request.status = EditStatus::Accepted;
            request.resolution = format!("accepted; landed upstream as {commit}");
            store::save(&root, &request)?;
            self.close_row(&request, ISSUE_DONE)?;
            landed.push(request.id);
        }
        Ok(landed)
    }

    fn close_row(&self, request: &EditRequest, status: &str) -> Result<(), WikiError> {
        let note = format!(
            "{} by {}{}",
            request.status.as_str(),
            request.resolved_by,
            if request.resolution.is_empty() {
                String::new()
            } else {
                format!(": {}", request.resolution)
            }
        );
        self.tracker
            .close_issue(request.id, status, &note)
            .map_err(|e| WikiError::Backend(format!("tracker: {e}")))
    }
}

fn refused(msg: String) -> WikiError {
    wide::set("wiki.edit.outcome", "refused");
    WikiError::Refused(msg)
}

fn must_be_open(request: &EditRequest) -> Result<(), WikiError> {
    if request.status.is_open() {
        Ok(())
    } else {
        Err(WikiError::IllegalState(format!(
            "edit request {} is {}",
            request.id,
            request.status.as_str()
        )))
    }
}

impl Edits for EditsBackend {
    /// t[impl wiki.edit.request] — any signed-in caller may open one,
    /// and opening writes the request and its tracker row, never a
    /// page.
    ///
    /// t[impl wiki.edit.tracked] — the row is opened with the request's
    /// own id and the `edit-request` tag.
    ///
    /// t[impl wiki.edit.auto-approve] — an Editor's own change lands
    /// within the same call unless they asked for review; it is still
    /// a request with a row.
    ///
    /// t[impl wiki.edit.gate] — `Closed` refuses with the state named;
    /// `Members` holds a request from someone the org does not vouch
    /// for rather than publishing it.
    fn open_edit_request(
        &self,
        wiki_id: &str,
        request: NewEditRequest,
    ) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        let proposer = self.principal()?;
        let config = self.wiki.config_of(wiki_id)?;
        if request.title.trim().is_empty() {
            return Err(WikiError::IllegalState(
                "an edit request needs a title".into(),
            ));
        }
        if request.changes.is_empty() {
            return Err(WikiError::IllegalState(
                "an edit request carries at least one page change".into(),
            ));
        }
        for change in &request.changes {
            // Same rules a direct write would meet: a curated page path,
            // under the wiki root. Reading it is how the path is checked.
            match self.wiki.read_page(wiki_id, &change.path) {
                Ok(_) | Err(WikiError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let held = match config.proposers {
            ProposerGate::Closed => {
                return Err(refused(format!(
                    "`{wiki_id}` has closed Edit Requests (proposers: {}); it will not look at \
                     new ones",
                    config.proposers.as_str()
                )));
            }
            ProposerGate::Members => !self.wiki.caller_is_org_member(),
            ProposerGate::Readers => false,
        };

        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let id = uuid::Uuid::new_v4();
        let now = self.now();
        let mut record = EditRequest {
            id,
            wiki: wiki_id.to_owned(),
            title: request.title.trim().to_owned(),
            summary: request.summary,
            proposer: proposer.clone(),
            opened_at: now.to_rfc3339(),
            status: EditStatus::Open,
            resolved_by: String::new(),
            resolved_at: String::new(),
            resolution: String::new(),
            claimed_by: String::new(),
            claimed_until: String::new(),
            auto_approved: false,
            held,
            landing: String::new(),
            changes: request.changes,
        };
        wide::set_display("wiki.edit.id", &id);
        self.tracker
            .open_issue(NewIssue {
                id,
                title: record.title.clone(),
                details: format!(
                    "Edit Request against wiki `{wiki_id}`, proposed by {proposer}.{}\n\nPages: {}",
                    if record.summary.trim().is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", record.summary.trim())
                    },
                    record
                        .changes
                        .iter()
                        .map(|c| c.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                tags: vec![
                    "task".into(),
                    EDIT_REQUEST_TAG.into(),
                    format!("wiki:{wiki_id}"),
                ],
            })
            .map_err(|e| WikiError::Backend(format!("tracker: {e}")))?;
        store::save(&root, &record)?;

        let mut outcome = if held { "held" } else { "opened" };
        if config.is_editor(&proposer) && !request.request_review && !held {
            match self.land(wiki_id, &mut record, &proposer) {
                Ok(()) => {
                    record.auto_approved = true;
                    store::save(&root, &record)?;
                    self.close_row(&record, ISSUE_DONE)?;
                    outcome = "auto_approved";
                }
                // An Editor's own change that no longer applies stays
                // open for them to look at, like anyone else's.
                Err(WikiError::Conflict(_)) => outcome = "conflict",
                Err(e) => return Err(e),
            }
        }
        wide::set("wiki.edit.outcome", outcome);
        Ok(record)
    }

    fn list_edit_requests(
        &self,
        wiki_id: &str,
        include_resolved: bool,
    ) -> Result<Vec<EditRequest>, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        let root = self.wiki.root_of(wiki_id)?;
        let config = self.wiki.config_of(wiki_id)?;
        let mut out = Vec::new();
        for mut request in store::list(&root)? {
            if self.reconcile(&mut request) {
                store::save(&root, &request)?;
            }
            if !include_resolved && !request.status.is_open() {
                continue;
            }
            if !self.may_see(&request, &config) {
                continue;
            }
            out.push(request);
        }
        Ok(out)
    }

    fn get_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let request = self.load(wiki_id, id)?;
        let config = self.wiki.config_of(wiki_id)?;
        if !self.may_see(&request, &config) {
            return Err(WikiError::NotFound(format!("edit request {id}")));
        }
        Ok(request)
    }

    /// t[impl wiki.edit.reviewable] — the request as a diff against the
    /// page as it is now.
    ///
    /// t[impl wiki.edit.rebase] — a stale page is merged three ways and
    /// reported as applying or not; the same view is shown to the
    /// reviewer and to the proposer.
    fn diff_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<Vec<PageDiff>, WikiError> {
        let request = self.get_edit_request(wiki_id, id)?;
        self.diffs(wiki_id, &request.changes)
    }

    fn revise_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        changes: Vec<PageChange>,
    ) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let principal = self.principal()?;
        if changes.is_empty() {
            return Err(WikiError::IllegalState(
                "a revision carries at least one page change".into(),
            ));
        }
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        if request.proposer != principal {
            return Err(refused("only the proposer revises a request".into()));
        }
        must_be_open(&request)?;
        request.changes = changes;
        request.status = EditStatus::Open;
        request.resolution.clear();
        request.resolved_by.clear();
        request.claimed_by.clear();
        request.claimed_until.clear();
        store::save(&root, &request)?;
        wide::set("wiki.edit.outcome", "revised");
        Ok(request)
    }

    /// t[impl wiki.edit.claim] — Editors only; refused while another
    /// Editor's unexpired claim stands; the claim carries its own expiry
    /// and reads back as none once past it.
    fn claim_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let (editor, _) = self.require_editor(wiki_id)?;
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        must_be_open(&request)?;
        if self.claimed_by_someone_else(&request, &editor) {
            return Err(refused(format!(
                "claimed by another Editor until {}",
                request.claimed_until
            )));
        }
        let until = self.now()
            + chrono::Duration::from_std(self.claim_ttl)
                .unwrap_or_else(|_| chrono::Duration::hours(1));
        request.claimed_by = editor;
        request.claimed_until = until.to_rfc3339();
        store::save(&root, &request)?;
        wide::set("wiki.edit.outcome", "claimed");
        Ok(request)
    }

    fn release_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
    ) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let (editor, _) = self.require_editor(wiki_id)?;
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        if self.claimed_by_someone_else(&request, &editor) {
            return Err(refused("another Editor holds this claim".into()));
        }
        request.claimed_by.clear();
        request.claimed_until.clear();
        store::save(&root, &request)?;
        wide::set("wiki.edit.outcome", "released");
        Ok(request)
    }

    /// t[impl wiki.edit.reviewable] — accepting lands every page as a
    /// version through the wiki's own write path and logs who proposed
    /// and who accepted.
    ///
    /// t[impl wiki.edit.rebase] — a stale request that merges cleanly
    /// lands merged; one that conflicts is refused with the pages named,
    /// nothing is written, and it stays open.
    ///
    /// t[impl wiki.edit.gate] — accepting a held request is the Editor
    /// vouching for it; nothing lands without this call.
    fn accept_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let (editor, _) = self.require_editor(wiki_id)?;
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        must_be_open(&request)?;
        if self.claimed_by_someone_else(&request, &editor) {
            return Err(refused(format!(
                "under review by another Editor until {}",
                request.claimed_until
            )));
        }
        self.land(wiki_id, &mut request, &editor)?;
        store::save(&root, &request)?;
        self.close_row(&request, ISSUE_DONE)?;
        wide::set("wiki.edit.outcome", "accepted");
        Ok(request)
    }

    /// t[impl wiki.edit.reviewable] — rejecting touches no page and
    /// keeps the request's text.
    fn reject_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        reason: &str,
    ) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let (editor, _) = self.require_editor(wiki_id)?;
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        must_be_open(&request)?;
        if self.claimed_by_someone_else(&request, &editor) {
            return Err(refused(format!(
                "under review by another Editor until {}",
                request.claimed_until
            )));
        }
        request.status = EditStatus::Rejected;
        request.resolved_by = editor;
        request.resolved_at = self.now().to_rfc3339();
        request.resolution = reason.trim().to_owned();
        request.claimed_by.clear();
        request.claimed_until.clear();
        store::save(&root, &request)?;
        self.close_row(&request, ISSUE_CANCELLED)?;
        wide::set("wiki.edit.outcome", "rejected");
        Ok(request)
    }

    fn return_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        reason: &str,
    ) -> Result<EditRequest, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        wide::set_display("wiki.edit.id", &id);
        let (editor, _) = self.require_editor(wiki_id)?;
        let _guard = self.guard();
        let root = self.wiki.root_of(wiki_id)?;
        let mut request = self.load(wiki_id, id)?;
        must_be_open(&request)?;
        if self.claimed_by_someone_else(&request, &editor) {
            return Err(refused(format!(
                "under review by another Editor until {}",
                request.claimed_until
            )));
        }
        request.status = EditStatus::Returned;
        // Not resolved — it is still waiting on somebody — but the
        // proposer needs to read who sent it back and why.
        request.resolution = format!("returned by {editor}: {}", reason.trim());
        request.claimed_by.clear();
        request.claimed_until.clear();
        store::save(&root, &request)?;
        wide::set("wiki.edit.outcome", "returned");
        Ok(request)
    }

    /// t[impl wiki.edit.editor] — who will review is readable before a
    /// request is opened, alongside who may open one.
    fn editors(&self, wiki_id: &str) -> Result<Editors, WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        let config = self.wiki.config_of(wiki_id)?;
        Ok(Editors {
            editors: config.editors,
            gate: config.proposers,
        })
    }

    /// t[impl wiki.edit.editor] — granted by an Editor of this wiki or
    /// an admin of the owning org; org membership alone confers
    /// nothing, including the right to declare the first Editor.
    fn grant_editor(&self, wiki_id: &str, principal: &str) -> Result<(), WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        let caller = self.principal()?;
        let principal = principal.trim();
        if principal.is_empty() {
            return Err(WikiError::IllegalState("an Editor is a principal".into()));
        }
        let config = self.wiki.config_of(wiki_id)?;
        let allowed = config.is_editor(&caller) || self.wiki.caller_is_org_admin();
        if !allowed {
            return Err(refused(if config.has_edit_lane() {
                format!("only an Editor of `{wiki_id}` or an org admin grants Editor")
            } else {
                format!(
                    "`{wiki_id}` has no Editors yet; an org admin declares the first one — \
                     membership alone does not"
                )
            }));
        }
        self.wiki.update_config(wiki_id, |c| {
            if !c.is_editor(principal) {
                c.editors.push(principal.to_owned());
            }
        })?;
        wide::set("wiki.edit.outcome", "editor_granted");
        Ok(())
    }

    /// t[impl wiki.edit.editor] — the last Editor cannot be revoked; a
    /// wiki that adopted the lane always has somebody who can land.
    fn revoke_editor(&self, wiki_id: &str, principal: &str) -> Result<(), WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        let caller = self.principal()?;
        let config = self.wiki.config_of(wiki_id)?;
        if !(config.is_editor(&caller) || self.wiki.caller_is_org_admin()) {
            return Err(refused(format!(
                "only an Editor of `{wiki_id}` or an org admin revokes Editor"
            )));
        }
        if !config.is_editor(principal) {
            return Err(WikiError::NotFound(format!(
                "`{principal}` is not an Editor"
            )));
        }
        if config.editors.len() == 1 {
            return Err(refused(format!(
                "`{principal}` is the last Editor of `{wiki_id}`; grant another before revoking"
            )));
        }
        self.wiki
            .update_config(wiki_id, |c| c.editors.retain(|e| e != principal))?;
        wide::set("wiki.edit.outcome", "editor_revoked");
        Ok(())
    }

    /// t[impl wiki.edit.gate] — the wiki declares who may propose,
    /// independently of who holds Editor.
    fn set_proposer_gate(&self, wiki_id: &str, gate: ProposerGate) -> Result<(), WikiError> {
        wide::set("wiki.slug", wiki_id.to_owned());
        self.require_editor(wiki_id)?;
        self.wiki.update_config(wiki_id, |c| c.proposers = gate)?;
        wide::set("wiki.edit.outcome", "gate_set");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::backend::Caller;

    /// A caller a test can switch between people.
    struct Switch {
        who: Mutex<Option<String>>,
        admin: Mutex<bool>,
        member: Mutex<bool>,
    }

    impl Caller for Switch {
        fn principal(&self) -> Option<String> {
            self.who.lock().unwrap().clone()
        }
        fn is_org_admin(&self) -> bool {
            *self.admin.lock().unwrap()
        }
        fn is_org_member(&self) -> bool {
            *self.member.lock().unwrap()
        }
    }

    struct World {
        _dir: tempfile::TempDir,
        root: PathBuf,
        switch: Arc<Switch>,
        tracker: Arc<MemoryTracker>,
        edits: EditsBackend,
        now: Arc<Mutex<DateTime<Utc>>>,
    }

    const IONIAN: &str = "Concepts/Ionian.md";
    const PAGE: &str = "# Ionian\n\nThe first mode.\n\n## See also\n\n- [[Modes]]\n";

    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let wikis = dir.path().join("wikis");
        let root = wikis.join("theory");
        std::fs::create_dir_all(root.join("Concepts")).unwrap();
        std::fs::write(root.join(IONIAN), PAGE).unwrap();
        std::fs::write(root.join("purpose.md"), "# Theory\n").unwrap();
        let mut config = WikiConfig::implicit("theory");
        config.editors.push("alice".into());
        crate::config::save(&root, &config).unwrap();

        let switch = Arc::new(Switch {
            who: Mutex::new(Some("sam".into())),
            admin: Mutex::new(false),
            member: Mutex::new(true),
        });
        let mut roots = HashMap::new();
        roots.insert("theory".to_string(), root.clone());
        let wiki = WikiBackend::with_roots_under(roots, wikis).with_caller(switch.clone());
        let tracker = Arc::new(MemoryTracker::default());
        let now = Arc::new(Mutex::new(Utc::now()));
        let clock_now = now.clone();
        let edits =
            EditsBackend::new(wiki, tracker.clone()).with_clock(move || *clock_now.lock().unwrap());
        World {
            _dir: dir,
            root,
            switch,
            tracker,
            edits,
            now,
        }
    }

    /// t[verify wiki.source.editable] — on a repo-sourced wiki an
    /// accepted request writes the working copy like anywhere else and
    /// reads `Accepted` at once; nothing reaches the repository, and the
    /// change shows up as a local change waiting for a push. An Editor's
    /// auto-approved edit does the same.
    #[test]
    fn accepting_on_a_repo_sourced_wiki_writes_the_working_copy_and_pushes_nothing() {
        use wiki_proto::service::registry::{ChangeKind, Registry as _};
        fn g(dir: &Path, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args(["-c", "user.email=t@example.com", "-c", "user.name=T"])
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(work.join("docs")).unwrap();
        g(&bare, &["init", "--bare", "--initial-branch=main"]);
        g(&work, &["init", "--initial-branch=main"]);
        std::fs::write(work.join("docs/Guide.md"), "# Guide\n\nOld text.\n").unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "docs"]);
        g(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        g(&work, &["push", "-u", "origin", "main"]);
        let main_before = g(&bare, &["rev-parse", "main"]);

        let wikis = dir.path().join("wikis");
        std::fs::create_dir_all(&wikis).unwrap();
        let switch = Arc::new(Switch {
            who: Mutex::new(Some("alice".into())),
            admin: Mutex::new(false),
            member: Mutex::new(true),
        });
        let wiki = WikiBackend::with_roots_under(HashMap::new(), wikis.clone())
            .with_caller(switch.clone());
        wiki.create_wiki(wiki_proto::config::NewWiki {
            title: "Docs".into(),
            source: Some(wiki_proto::config::RepoSource {
                url: format!("file://{}", bare.display()),
                branch: "main".into(),
                path: "docs".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let tracker = Arc::new(MemoryTracker::default());
        let edits = EditsBackend::new(wiki.clone(), tracker.clone());
        let before = wiki.read_page("docs", "Guide.md").unwrap();

        // Sam proposes; Alice (the creator, hence Editor) accepts.
        *switch.who.lock().unwrap() = Some("sam".into());
        let req = edits
            .open_edit_request(
                "docs",
                NewEditRequest {
                    title: "Clarify the guide".into(),
                    summary: String::new(),
                    changes: vec![PageChange {
                        path: "Guide.md".into(),
                        base_sha256: before.sha256.clone(),
                        base_markdown: before.markdown.clone(),
                        markdown: "# Guide\n\nNew text.\n".into(),
                        delete: false,
                    }],
                    request_review: false,
                },
            )
            .unwrap();
        *switch.who.lock().unwrap() = Some("alice".into());
        let accepted = edits.accept_edit_request("docs", req.id).unwrap();
        assert_eq!(accepted.status, EditStatus::Accepted, "{accepted:?}");
        assert!(accepted.landing.is_empty());
        assert_eq!(
            wiki.read_page("docs", "Guide.md").unwrap().markdown,
            "# Guide\n\nNew text.\n",
            "the working copy has the change"
        );
        assert_eq!(
            tracker.issue_status(req.id).unwrap().as_deref(),
            Some("done")
        );
        assert_eq!(g(&bare, &["branch", "--list"]), "* main", "nothing pushed");
        assert_eq!(g(&bare, &["rev-parse", "main"]), main_before);
        assert!(edits.reconcile_landings("docs").unwrap().is_empty());

        // Alice's own edit, auto-approved, lands the same way.
        let current = wiki.read_page("docs", "Guide.md").unwrap();
        let mine = edits
            .open_edit_request(
                "docs",
                NewEditRequest {
                    title: "Add a page".into(),
                    summary: String::new(),
                    changes: vec![PageChange {
                        path: "Deploy.md".into(),
                        base_sha256: String::new(),
                        base_markdown: String::new(),
                        markdown: "# Deploy\n".into(),
                        delete: false,
                    }],
                    request_review: false,
                },
            )
            .unwrap();
        assert_eq!(mine.status, EditStatus::Accepted);
        assert!(mine.auto_approved);
        assert_eq!(current.markdown, "# Guide\n\nNew text.\n");

        // Both are local changes now, waiting for a push.
        let changes = wiki.local_changes("docs").unwrap();
        let kinds: Vec<(&str, ChangeKind)> = changes
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("Deploy.md", ChangeKind::Added),
                ("Guide.md", ChangeKind::Modified)
            ]
        );
        assert!(changes.pending.is_none());
        assert_eq!(
            g(&bare, &["branch", "--list"]),
            "* main",
            "still nothing pushed"
        );
    }

    impl World {
        fn as_(&self, who: &str) {
            *self.switch.who.lock().unwrap() = Some(who.to_owned());
        }
        fn as_server(&self) {
            *self.switch.who.lock().unwrap() = None;
        }
        fn page(&self) -> String {
            std::fs::read_to_string(self.root.join(IONIAN)).unwrap()
        }
        fn sha(s: &str) -> String {
            crate::backend::sha256_hex(s.as_bytes())
        }
        fn proposal(&self, markdown: &str) -> NewEditRequest {
            NewEditRequest {
                title: "Clarify the leading tone".into(),
                summary: "Says what the 7th does.".into(),
                changes: vec![PageChange {
                    path: IONIAN.into(),
                    base_sha256: Self::sha(&self.page()),
                    base_markdown: self.page(),
                    markdown: markdown.into(),
                    delete: false,
                }],
                request_review: false,
            }
        }
        fn advance(&self, secs: i64) {
            let mut now = self.now.lock().unwrap();
            *now += chrono::Duration::seconds(secs);
        }
    }

    const SAMS: &str =
        "# Ionian\n\nThe first mode. Its 7th pulls home.\n\n## See also\n\n- [[Modes]]\n";

    fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if rel.starts_with("_state/") {
                continue;
            }
            out.push((rel, std::fs::read(p).unwrap()));
        }
        out.sort();
        out
    }

    /// t[verify wiki.edit.request] — opening writes no page.
    /// t[verify wiki.edit.tracked] — the row exists with the request's
    /// id and the `edit-request` tag.
    #[test]
    fn opening_records_a_request_and_a_row_and_changes_no_page() {
        let w = world();
        let before = snapshot(&w.root);
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        assert_eq!(r.status, EditStatus::Open);
        assert_eq!(r.proposer, "sam");
        assert!(!r.auto_approved && !r.held);
        assert_eq!(snapshot(&w.root), before, "opening mutated the wiki");
        let listed = w.edits.list_edit_requests("theory", false).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, r.id);
        let rows = w.tracker.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.id, r.id);
        assert!(rows[0].0.tags.iter().any(|t| t == EDIT_REQUEST_TAG));
        assert!(rows[0].0.tags.iter().any(|t| t == "wiki:theory"));
    }

    /// t[verify wiki.edit.request] — a caller with no principal cannot
    /// open one; the lane records who.
    #[test]
    fn a_request_needs_a_proposer() {
        let w = world();
        w.as_server();
        let err = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap_err();
        assert!(matches!(err, WikiError::Refused(_)), "{err:?}");
    }

    /// t[verify wiki.edit.reviewable] — a diff, then a landing
    /// attributed to the proposer; the row is done.
    /// t[verify wiki.edit.claim] — the Editor claims first, and the
    /// claim is visible on the request.
    #[test]
    fn an_editor_claims_reviews_and_accepts() {
        let w = world();
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        let err = w.edits.accept_edit_request("theory", r.id).unwrap_err();
        assert!(
            matches!(err, WikiError::Refused(_)),
            "sam accepted his own: {err:?}"
        );

        w.as_("alice");
        let claimed = w.edits.claim_edit_request("theory", r.id).unwrap();
        assert_eq!(claimed.claimed_by, "alice");
        assert!(!claimed.claimed_until.is_empty());
        let diffs = w.edits.diff_edit_request("theory", r.id).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].current, PAGE);
        assert_eq!(diffs[0].proposed, SAMS);
        assert!(!diffs[0].stale && diffs[0].applies);
        assert_eq!(diffs[0].merged, SAMS);

        let accepted = w.edits.accept_edit_request("theory", r.id).unwrap();
        assert_eq!(accepted.status, EditStatus::Accepted);
        assert_eq!(accepted.resolved_by, "alice");
        assert!(accepted.claimed_by.is_empty());
        assert_eq!(w.page(), SAMS);
        let log = std::fs::read_to_string(w.root.join("log.md")).unwrap();
        assert!(
            log.contains("sam"),
            "log does not name the proposer:\n{log}"
        );
        assert!(
            log.contains(&r.id.to_string()),
            "log does not name the request:\n{log}"
        );
        assert_eq!(w.tracker.rows()[0].1, ISSUE_DONE);
        assert!(
            w.edits
                .list_edit_requests("theory", false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(w.edits.list_edit_requests("theory", true).unwrap().len(), 1);
    }

    /// t[verify wiki.edit.reviewable] — rejecting leaves the wiki
    /// byte-identical and keeps the request's text.
    #[test]
    fn rejecting_touches_nothing_and_keeps_the_text() {
        let w = world();
        let before = snapshot(&w.root);
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        w.as_("alice");
        let rejected = w
            .edits
            .reject_edit_request("theory", r.id, "not here")
            .unwrap();
        assert_eq!(rejected.status, EditStatus::Rejected);
        assert_eq!(rejected.resolution, "not here");
        assert_eq!(rejected.changes[0].markdown, SAMS);
        assert_eq!(snapshot(&w.root), before);
        assert_eq!(w.tracker.rows()[0].1, ISSUE_CANCELLED);
        // Returned lets the proposer revise, and revising reopens —
        // but only an Editor returns it.
        w.as_("sam");
        let r2 = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        let err = w
            .edits
            .return_edit_request("theory", r2.id, "x")
            .unwrap_err();
        assert!(matches!(err, WikiError::Refused(_)));
        w.as_("alice");
        let returned = w
            .edits
            .return_edit_request("theory", r2.id, "cite it")
            .unwrap();
        assert_eq!(returned.status, EditStatus::Returned);
        assert!(returned.resolution.contains("cite it"));
        w.as_("sam");
        let mut changes = returned.changes.clone();
        changes[0].markdown = format!("{SAMS}\nCited.\n");
        let revised = w
            .edits
            .revise_edit_request("theory", r2.id, changes)
            .unwrap();
        assert_eq!(revised.status, EditStatus::Open);
        assert_eq!(snapshot(&w.root), before);
    }

    /// t[verify wiki.edit.rebase] — a stale request that does not
    /// overlap lands merged; one that overlaps is a `Conflict` naming the
    /// page, nothing is written, and the request stays open.
    #[test]
    fn a_stale_request_merges_or_conflicts_never_by_recency() {
        let w = world();
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        // Alice changes a different region directly (she is an Editor).
        w.as_("alice");
        let alices = PAGE.replace("- [[Modes]]\n", "- [[Modes]]\n- [[Harmonic Series]]\n");
        let current = w.edits.wiki().read_page("theory", IONIAN).unwrap();
        w.edits
            .wiki()
            .write_page("theory", IONIAN, &alices, &current.sha256)
            .unwrap();
        let diffs = w.edits.diff_edit_request("theory", r.id).unwrap();
        assert!(diffs[0].stale && diffs[0].applies, "{:?}", diffs[0]);
        let accepted = w.edits.accept_edit_request("theory", r.id).unwrap();
        assert_eq!(accepted.status, EditStatus::Accepted);
        let page = w.page();
        assert!(page.contains("Its 7th pulls home."), "{page}");
        assert!(page.contains("- [[Harmonic Series]]"), "{page}");

        // Same line, both sides: a person decides.
        w.as_("sam");
        let base = w.page();
        let r2 = w
            .edits
            .open_edit_request(
                "theory",
                NewEditRequest {
                    title: "Reword".into(),
                    summary: String::new(),
                    changes: vec![PageChange {
                        path: IONIAN.into(),
                        base_sha256: World::sha(&base),
                        base_markdown: base.clone(),
                        markdown: base.replace("Its 7th pulls home.", "Its seventh leads home."),
                        delete: false,
                    }],
                    request_review: false,
                },
            )
            .unwrap();
        w.as_("alice");
        let cur = w.edits.wiki().read_page("theory", IONIAN).unwrap();
        let alices2 = base.replace("Its 7th pulls home.", "Its 7th resolves upward.");
        w.edits
            .wiki()
            .write_page("theory", IONIAN, &alices2, &cur.sha256)
            .unwrap();
        let before = snapshot(&w.root);
        let err = w.edits.accept_edit_request("theory", r2.id).unwrap_err();
        match err {
            WikiError::Conflict(msg) => assert!(msg.contains(IONIAN), "{msg}"),
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(
            snapshot(&w.root),
            before,
            "a conflicting accept wrote something"
        );
        let still = w.edits.get_edit_request("theory", r2.id).unwrap();
        assert_eq!(still.status, EditStatus::Open);
        let d = w.edits.diff_edit_request("theory", r2.id).unwrap();
        assert!(d[0].stale && !d[0].applies);
    }

    /// t[verify wiki.edit.tracked] — closing the row from the issue
    /// side closes the request; nothing lands.
    #[test]
    fn closing_the_issue_closes_the_request() {
        let w = world();
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        w.tracker.set_status(r.id, "done");
        let seen = w.edits.get_edit_request("theory", r.id).unwrap();
        assert_eq!(seen.status, EditStatus::Closed);
        assert_eq!(w.page(), PAGE);
        assert!(
            w.edits
                .list_edit_requests("theory", false)
                .unwrap()
                .is_empty()
        );
        w.as_("alice");
        let err = w.edits.accept_edit_request("theory", r.id).unwrap_err();
        assert!(matches!(err, WikiError::IllegalState(_)), "{err:?}");
    }

    /// t[verify wiki.edit.auto-approve] — an Editor's own change lands
    /// in the same call, recorded with a row; with review requested it
    /// stays open.
    #[test]
    fn an_editors_change_is_approved_within_the_lane() {
        let w = world();
        w.as_("alice");
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        assert_eq!(r.status, EditStatus::Accepted);
        assert!(r.auto_approved);
        assert_eq!(r.resolved_by, "alice");
        assert_eq!(w.page(), SAMS);
        assert_eq!(w.tracker.rows().len(), 1);
        assert_eq!(w.tracker.rows()[0].1, ISSUE_DONE);
        let all = w.edits.list_edit_requests("theory", true).unwrap();
        assert_eq!(all.len(), 1, "every change and who made it is one query");

        let mut reviewed = w.proposal(PAGE);
        reviewed.request_review = true;
        let r2 = w.edits.open_edit_request("theory", reviewed).unwrap();
        assert_eq!(r2.status, EditStatus::Open);
        assert!(!r2.auto_approved);
        assert_eq!(w.page(), SAMS);
    }

    /// t[verify wiki.edit.gate] — `Closed` refuses naming the state;
    /// `Members` holds a non-member's request, which Editors see and
    /// nobody publishes until one accepts.
    #[test]
    fn the_gate_refuses_or_holds() {
        let w = world();
        w.as_("sam");
        let err = w
            .edits
            .set_proposer_gate("theory", ProposerGate::Closed)
            .unwrap_err();
        assert!(
            matches!(err, WikiError::Refused(_)),
            "a non-Editor set the gate"
        );
        w.as_("alice");
        w.edits
            .set_proposer_gate("theory", ProposerGate::Closed)
            .unwrap();
        assert_eq!(
            w.edits.editors("theory").unwrap().gate,
            ProposerGate::Closed
        );
        w.as_("sam");
        let err = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap_err();
        match err {
            WikiError::Refused(msg) => assert!(msg.contains("closed"), "{msg}"),
            other => panic!("{other:?}"),
        }

        w.as_("alice");
        w.edits
            .set_proposer_gate("theory", ProposerGate::Members)
            .unwrap();
        *w.switch.member.lock().unwrap() = false;
        w.as_("outsider");
        let held = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        assert!(held.held);
        assert_eq!(w.page(), PAGE);
        // Held requests are not listed to just anyone...
        w.as_("sam");
        assert!(
            w.edits
                .list_edit_requests("theory", false)
                .unwrap()
                .is_empty()
        );
        // ...but Editors see them and may vouch by accepting.
        w.as_("alice");
        assert_eq!(
            w.edits.list_edit_requests("theory", false).unwrap().len(),
            1
        );
        // An Editor who is not a member is still held, never auto-approved.
        let mine = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        assert!(mine.held && !mine.auto_approved);
        assert_eq!(mine.status, EditStatus::Open);
        assert_eq!(w.page(), PAGE);
        let accepted = w.edits.accept_edit_request("theory", held.id).unwrap();
        assert_eq!(accepted.status, EditStatus::Accepted);
        assert_eq!(w.page(), SAMS);
    }

    /// t[verify wiki.edit.claim] — two Editors: the second is refused
    /// while the first's claim stands, may claim once it has expired, and
    /// a release gives it back early.
    #[test]
    fn a_claim_excludes_and_expires() {
        let w = world();
        let r = w
            .edits
            .open_edit_request("theory", w.proposal(SAMS))
            .unwrap();
        w.as_("alice");
        w.edits.grant_editor("theory", "bob").unwrap();
        w.edits.claim_edit_request("theory", r.id).unwrap();
        w.as_("bob");
        let err = w.edits.claim_edit_request("theory", r.id).unwrap_err();
        assert!(matches!(err, WikiError::Refused(_)), "{err:?}");
        let err = w.edits.accept_edit_request("theory", r.id).unwrap_err();
        assert!(
            matches!(err, WikiError::Refused(_)),
            "accepted under another's claim"
        );
        w.advance(DEFAULT_CLAIM_TTL.as_secs() as i64 + 1);
        let seen = w.edits.get_edit_request("theory", r.id).unwrap();
        assert!(seen.claimed_by.is_empty(), "an expired claim still shows");
        assert_eq!(seen.status, EditStatus::Open, "expiry lost the request");
        let claimed = w.edits.claim_edit_request("theory", r.id).unwrap();
        assert_eq!(claimed.claimed_by, "bob");
        let released = w.edits.release_edit_request("theory", r.id).unwrap();
        assert!(released.claimed_by.is_empty());
        w.as_("alice");
        w.edits.claim_edit_request("theory", r.id).unwrap();
    }

    /// t[verify wiki.edit.editor] — Editor is granted by an Editor or an
    /// org admin, never by membership; the last one cannot be revoked;
    /// a wiki with none takes its first from an admin only.
    #[test]
    fn editor_is_a_role_on_one_wiki() {
        let w = world();
        w.as_("sam");
        let err = w.edits.grant_editor("theory", "sam").unwrap_err();
        assert!(
            matches!(err, WikiError::Refused(_)),
            "a member granted himself"
        );
        w.as_("alice");
        let err = w.edits.revoke_editor("theory", "alice").unwrap_err();
        assert!(
            matches!(err, WikiError::Refused(_)),
            "the last Editor was revoked"
        );
        w.edits.grant_editor("theory", "sam").unwrap();
        w.edits.grant_editor("theory", "sam").unwrap();
        assert_eq!(
            w.edits.editors("theory").unwrap().editors,
            vec!["alice", "sam"]
        );
        w.edits.revoke_editor("theory", "alice").unwrap();
        assert_eq!(w.edits.editors("theory").unwrap().editors, vec!["sam"]);

        // A wiki with no Editors: a member cannot bootstrap; an admin can.
        let root2 = w.root.parent().unwrap().join("second");
        std::fs::create_dir_all(&root2).unwrap();
        crate::config::save(&root2, &WikiConfig::implicit("second")).unwrap();
        let mut roots = HashMap::new();
        roots.insert("second".to_string(), root2);
        let wiki2 = WikiBackend::with_roots(roots).with_caller(w.switch.clone());
        let edits2 = EditsBackend::new(wiki2, Arc::new(MemoryTracker::default()));
        w.as_("sam");
        let err = edits2.grant_editor("second", "sam").unwrap_err();
        assert!(matches!(err, WikiError::Refused(_)));
        *w.switch.admin.lock().unwrap() = true;
        edits2.grant_editor("second", "sam").unwrap();
        assert_eq!(edits2.editors("second").unwrap().editors, vec!["sam"]);
    }

    /// t[verify wiki.edit.editor] — once a wiki has Editors, a direct
    /// write from anyone else is refused; the server's own in-process
    /// writes still land.
    #[test]
    fn direct_writes_are_for_editors_once_the_lane_is_on() {
        let w = world();
        w.as_("sam");
        let cur = w.edits.wiki().read_page("theory", IONIAN).unwrap();
        let err = w
            .edits
            .wiki()
            .write_page("theory", IONIAN, SAMS, &cur.sha256)
            .unwrap_err();
        match err {
            WikiError::Refused(msg) => assert!(msg.contains("Edit Request"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(w.page(), PAGE);
        w.as_server();
        w.edits
            .wiki()
            .write_page("theory", IONIAN, SAMS, &cur.sha256)
            .unwrap();
        assert_eq!(w.page(), SAMS);
    }
}
