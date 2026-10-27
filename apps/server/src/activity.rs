//! The activity feed's server half: who did it, what changed, and the
//! comment hook.
//!
//! The feed itself is `notify::ActivityStore` (history beside the
//! notifications queue in `notify.sqlite`); the rules that turn hub
//! events into entries live here as pure functions ([`task_changes`],
//! [`inbox_change`], [`timer_change`], [`project_change`]) so they are
//! unit-tested without a server, and `crate::notifier` drives them from
//! the same pumps that raise notifications.
//!
//! # Who did it
//!
//! The change streams carry state, not authors: a `TaskEvent::Upserted`
//! says what the task now is, never who made it so. Two seams recover
//! the author for writes that arrive over the org router:
//!
//! - **Sync services** (`TaskService`, `Inbox`) run their methods on a
//!   blocking thread, off the request task where the permission gate's
//!   caller lives. [`ActorDispatcher`] reads the caller *on* the
//!   request task and hands it to the blocking thread; the decorators
//!   ([`AttributedTasks`], [`AttributedInbox`]) note it in the
//!   [`ActorLedger`] against the entity before the write publishes.
//! - **Async services** (`TimerService`, `ThreadsService`) run on the
//!   request task, so the gate's caller is readable directly.
//!
//! The notifier asks the ledger who touched an entity when it sees the
//! change. Writes that do not come through the router — the forge poll
//! loop, an editor saving the markdown, the CLI talking to the vault —
//! have no caller, and fall back as `docs/spec/unmet.md` § activity
//! says.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use architect::dispatch::{BoxedAny, DispatchError, Dispatcher};
use chrono::{DateTime, Utc};
use notify::{ActivityStore, NewActivity};
use notify_proto::ActivityKind;
use uuid::Uuid;

// ── who is calling ──────────────────────────────────────────────────

thread_local! {
    /// The caller of the sync RPC running on this blocking thread, set
    /// by [`ActorDispatcher`] for the closure's duration.
    static BLOCKING_CALLER: RefCell<Option<String>> = const { RefCell::new(None) };
    /// That caller as a creator (`crate::attribution`), resolved on the
    /// request task by a dispatcher that carries a directory.
    static BLOCKING_CREATOR: RefCell<Option<crate::attribution::Creator>> =
        const { RefCell::new(None) };
}

/// The signed-in user (auth user id) behind the RPC running here, when
/// the permission gate resolved one. Works on the request task (async
/// services) and inside an [`ActorDispatcher`] closure (sync ones).
#[must_use]
pub fn caller_user() -> Option<String> {
    if let Some(architect_permissions::Principal::User { user_id }) =
        architect::permissions_gate::caller()
    {
        return Some(user_id);
    }
    BLOCKING_CALLER.with(|c| c.borrow().clone())
}

/// The creator the sync RPC running on this thread is attributed to:
/// its gate caller, resolved by the dispatcher before the hop. `None`
/// when no person or bot stands behind the call.
#[must_use]
pub fn caller_creator() -> Option<crate::attribution::Creator> {
    BLOCKING_CREATOR.with(|c| c.borrow().clone())
}

/// Clears [`BLOCKING_CALLER`] however the closure ends — blocking
/// threads are pooled, and a panicking write must not leave its caller
/// behind for the next one.
struct ClearOnDrop;
impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        BLOCKING_CALLER.with(|c| *c.borrow_mut() = None);
        BLOCKING_CREATOR.with(|c| *c.borrow_mut() = None);
    }
}

/// `spawn_blocking`, like architect's `TokioBlockingDispatcher`, but
/// carrying the gate's caller across the thread hop — and, given a
/// [`crate::attribution::Creators`] directory, who that caller is, so
/// a create can be attributed without an async lookup on the blocking
/// thread.
///
/// The caller is read inside the returned future — that future is
/// polled within the gate's scope, which is where the caller is set.
#[derive(Default, Clone)]
pub struct ActorDispatcher {
    creators: Option<crate::attribution::Creators>,
}

impl ActorDispatcher {
    /// A dispatcher that also resolves the caller as a creator.
    #[must_use]
    pub fn attributing(creators: crate::attribution::Creators) -> Self {
        Self {
            creators: Some(creators),
        }
    }
}

impl std::fmt::Debug for ActorDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorDispatcher")
            .field("attributing", &self.creators.is_some())
            .finish()
    }
}

impl Dispatcher for ActorDispatcher {
    fn dispatch(
        &self,
        f: Box<dyn FnOnce() -> BoxedAny + Send + 'static>,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedAny, DispatchError>> + Send + 'static>> {
        let creators = self.creators.clone();
        Box::pin(async move {
            let who = caller_user();
            let creator = match (&creators, &who) {
                (Some(dir), Some(id)) => Some(dir.resolve(id).await),
                _ => None,
            };
            tokio::task::spawn_blocking(move || {
                let _clear = ClearOnDrop;
                BLOCKING_CALLER.with(|c| *c.borrow_mut() = who);
                BLOCKING_CREATOR.with(|c| *c.borrow_mut() = creator);
                f()
            })
            .await
            .map_err(|e| {
                if e.is_panic() {
                    DispatchError::Panicked(format!("{e:?}"))
                } else {
                    DispatchError::Cancelled
                }
            })
        })
    }
}

/// Who last wrote each entity over the router, briefly.
///
/// Keys are `"<entity_type>:<id>"`, plus `"task:new"` for a create
/// whose id the backend mints and `"timer-user:<user_id>"` for the
/// timer (a session's id does not exist until it starts). Entries go
/// stale after [`ActorLedger::TTL`]: the notifier reads them within
/// milliseconds of the write, and an old entry must not name the author
/// of a later change that came from somewhere with no caller.
#[derive(Clone, Default)]
pub struct ActorLedger(Arc<Mutex<HashMap<String, (String, Instant)>>>);

impl ActorLedger {
    pub const TTL: Duration = Duration::from_secs(30);

    /// Record that `user` (when known) is writing `key` now.
    pub fn note(&self, key: impl Into<String>, user: Option<String>) {
        let Some(user) = user else { return };
        let mut map = self.0.lock().expect("actor ledger lock");
        // Bounded by the number of entities written in the last TTL;
        // sweep the stale ones whenever it grows.
        if map.len() > 512 {
            map.retain(|_, (_, at)| at.elapsed() < Self::TTL);
        }
        map.insert(key.into(), (user, Instant::now()));
    }

    /// Who wrote `key` within the TTL.
    #[must_use]
    pub fn who(&self, key: &str) -> Option<String> {
        let map = self.0.lock().expect("actor ledger lock");
        map.get(key)
            .filter(|(_, at)| at.elapsed() < Self::TTL)
            .map(|(u, _)| u.clone())
    }

    /// [`Self::who`] for `key`, else for `fallback` (a create's
    /// `"task:new"`).
    #[must_use]
    pub fn who_or(&self, key: &str, fallback: &str) -> Option<String> {
        self.who(key).or_else(|| self.who(fallback))
    }
}

/// User id → display name, from the org's auth store. Refreshed by the
/// notifier on a slow loop; a miss names nobody rather than blocking.
#[derive(Clone, Default)]
pub struct People(Arc<RwLock<HashMap<String, String>>>);

impl People {
    /// Reload from the org's auth store.
    pub async fn refresh(&self, auth: &crate::AuthState) {
        let Ok(users) = auth.auth.list_users_local_trusted().await else {
            return;
        };
        let names: HashMap<String, String> = users
            .into_iter()
            .map(|u| {
                let name = u
                    .name
                    .filter(|n| !n.trim().is_empty())
                    .or(u.display_username)
                    .or(u.email)
                    .unwrap_or_else(|| "Someone".to_owned());
                (u.id.to_string(), name)
            })
            .collect();
        *self.0.write().expect("people lock") = names;
    }

    #[must_use]
    pub fn name(&self, id: &str) -> Option<String> {
        self.0.read().expect("people lock").get(id).cloned()
    }

    /// For tests and the seed.
    pub fn insert(&self, id: impl Into<String>, name: impl Into<String>) {
        self.0
            .write()
            .expect("people lock")
            .insert(id.into(), name.into());
    }
}

/// An actor as an entry carries one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Actor {
    pub id: String,
    pub name: String,
}

impl Actor {
    /// Nobody the server could name.
    #[must_use]
    pub fn someone() -> Self {
        Self {
            id: String::new(),
            name: "Someone".to_owned(),
        }
    }

    /// A known user, named from `people` (their id when unnamed).
    #[must_use]
    pub fn user(id: &str, people: &People) -> Self {
        Self {
            id: id.to_owned(),
            name: people.name(id).unwrap_or_else(|| "Someone".to_owned()),
        }
    }

    fn stamp(&self, n: &mut NewActivity) {
        n.actor_id.clone_from(&self.id);
        n.actor_name.clone_from(&self.name);
    }
}

// ── the writer ──────────────────────────────────────────────────────

/// The notifier's one writer: rule hits arrive in order on a channel
/// and land in the store in that order (each carries its own `at`).
#[derive(Clone)]
pub struct Recorder {
    tx: tokio::sync::mpsc::UnboundedSender<NewActivity>,
}

impl Recorder {
    /// Start the writer task for one org's store.
    #[must_use]
    pub fn spawn(store: ActivityStore, slug: String) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NewActivity>();
        tokio::spawn(async move {
            while let Some(new) = rx.recv().await {
                if let Err(e) = store.record(new).await {
                    // A lost history line is worth one alertable line.
                    tracing::warn!(org = %slug, error = %e, "activity: write failed");
                }
            }
        });
        Self { tx }
    }

    pub fn record(&self, new: NewActivity) {
        let _ = self.tx.send(new);
    }

    pub fn record_all(&self, all: impl IntoIterator<Item = NewActivity>) {
        for n in all {
            self.record(n);
        }
    }
}

// ── task rules ──────────────────────────────────────────────────────

/// What the feed remembers per task to see a change in a full-state
/// `Upserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnap {
    pub title: String,
    pub status: String,
    pub priority: String,
    pub due: Option<String>,
    /// `(id, name)` of the primary assignee.
    pub assignee: Option<(String, String)>,
    pub details_len: usize,
    pub details_hash: u64,
    pub project_id: Option<Uuid>,
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `(id, name)` for an assignee — a person's id and display name, or
/// an agent's label twice.
#[must_use]
pub fn agent_ref(a: &task::workflows_proto::AgentRef, people: &People) -> (String, String) {
    match a {
        task::workflows_proto::AgentRef::Human {
            user_id,
            display_name,
        } => (
            user_id.clone(),
            display_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .or_else(|| people.name(user_id))
                .unwrap_or_else(|| user_id.clone()),
        ),
        other => {
            let l = other.short_label();
            (l.clone(), l)
        }
    }
}

impl TaskSnap {
    #[must_use]
    pub fn of(t: &task::TaskInfo, people: &People) -> Self {
        Self {
            title: t.title.clone(),
            status: t.status.clone(),
            priority: t.priority.clone(),
            due: t.due.clone(),
            assignee: t
                .workflow
                .as_ref()
                .and_then(|w| w.assignees.0.first())
                .map(|a| agent_ref(a, people)),
            details_len: t.details.len(),
            details_hash: hash_str(&t.details),
            project_id: t.project_id,
        }
    }
}

/// An editing burst is one entry: a description saved again by the
/// same person within this window does not say "edited" twice.
pub const EDIT_COALESCE: Duration = Duration::from_secs(10 * 60);

/// The entries one task change makes. `before = None` is a task the
/// feed has not seen — a creation (the caller only passes `None` for
/// ids first seen *after* the seed). `last_edit` is when this task's
/// description last made an "edited" entry, for coalescing.
// t[impl activity.task.diff]
#[must_use]
pub fn task_changes(
    before: Option<&TaskSnap>,
    after: &task::TaskInfo,
    now_snap: &TaskSnap,
    actor: &Actor,
    last_edit: Option<Instant>,
) -> Vec<NewActivity> {
    let base = |kind: ActivityKind| {
        let mut n = NewActivity::new(kind, "task", after.id);
        n.entity_title.clone_from(&after.title);
        n.href = format!("/tasks/{}", after.id);
        n.project_id = after.project_id;
        actor.stamp(&mut n);
        n
    };
    let change = |kind: ActivityKind, field: &str, from: &str, to: &str| {
        let mut n = base(kind);
        n.field = field.to_owned();
        n.from = from.to_owned();
        n.to = to.to_owned();
        n
    };
    let Some(b) = before else {
        return vec![base(ActivityKind::Created)];
    };
    let mut out = Vec::new();
    if b.title != now_snap.title {
        let mut n = change(ActivityKind::Renamed, "title", &b.title, &now_snap.title);
        // The entry names the task by what it is now.
        n.entity_title.clone_from(&now_snap.title);
        out.push(n);
    }
    if !b.status.eq_ignore_ascii_case(&now_snap.status) {
        let was = task::status_is_terminal(&b.status);
        let is = task::status_is_terminal(&now_snap.status);
        let kind = match (was, is) {
            (false, true) => ActivityKind::Completed,
            (true, false) => ActivityKind::Reopened,
            _ => ActivityKind::StatusChanged,
        };
        out.push(change(kind, "status", &b.status, &now_snap.status));
    }
    if b.assignee != now_snap.assignee {
        match (&b.assignee, &now_snap.assignee) {
            (_, Some((_, to))) => {
                let from = b.assignee.as_ref().map_or("", |(_, n)| n.as_str());
                out.push(change(ActivityKind::Assigned, "assignee", from, to));
            }
            (Some((_, from)), None) => {
                out.push(change(ActivityKind::Unassigned, "assignee", from, ""));
            }
            (None, None) => {}
        }
    }
    if !b.priority.eq_ignore_ascii_case(&now_snap.priority) {
        out.push(change(
            ActivityKind::PriorityChanged,
            "priority",
            &b.priority,
            &now_snap.priority,
        ));
    }
    if b.due != now_snap.due {
        out.push(change(
            ActivityKind::DueChanged,
            "due",
            b.due.as_deref().unwrap_or(""),
            now_snap.due.as_deref().unwrap_or(""),
        ));
    }
    let edited = b.details_hash != now_snap.details_hash;
    if edited && last_edit.is_none_or(|t| t.elapsed() >= EDIT_COALESCE) {
        out.push(change(ActivityKind::Edited, "details", "", ""));
    }
    out
}

/// The entry a task's deletion makes, from what the feed last knew.
#[must_use]
pub fn task_deleted(id: Uuid, before: &TaskSnap, actor: &Actor) -> NewActivity {
    let mut n = NewActivity::new(ActivityKind::Deleted, "task", id);
    n.entity_title.clone_from(&before.title);
    n.project_id = before.project_id;
    actor.stamp(&mut n);
    n
}

/// Who a task change is attributed to: the router caller the ledger
/// saw, else (for writes with no caller) the task's assignee — which
/// is right for a claim and a guess otherwise — else nobody.
#[must_use]
pub fn task_actor(
    ledger: &ActorLedger,
    people: &People,
    id: Uuid,
    snap: Option<&TaskSnap>,
) -> Actor {
    if let Some(u) = ledger.who_or(&format!("task:{id}"), "task:new") {
        return Actor::user(&u, people);
    }
    match snap.and_then(|s| s.assignee.clone()) {
        Some((id, name)) => Actor { id, name },
        None => Actor {
            id: String::new(),
            name: "Someone".to_owned(),
        },
    }
}

// ── inbox rules ─────────────────────────────────────────────────────

/// What the feed remembers per capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxSnap {
    pub status: String,
}

/// A capture's headline — its first line, trimmed.
#[must_use]
pub fn capture_title(body: &str) -> String {
    let line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim().trim_start_matches('#').trim();
    if line.chars().count() > 120 {
        let cut: String = line.chars().take(119).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// What an inbox change means for the feed. `Captured` for an id
/// first seen open; `Processed` (unresolved — see [`ProcessedInto`])
/// when an open capture turns processed.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxChange {
    Captured(NewActivity),
    /// The entry, plus the provenance link to resolve into a title and
    /// an href (it names a task path, `project:<id>`, or a note path).
    Processed(NewActivity, Option<String>),
}

// t[impl activity.inbox]
#[must_use]
pub fn inbox_change(
    before: Option<&InboxSnap>,
    item: &inbox_proto::InboxItem,
    actor: &Actor,
) -> Option<InboxChange> {
    let base = |kind: ActivityKind| {
        let mut n = NewActivity::new(kind, "inbox", &item.id);
        n.entity_title = capture_title(&item.body);
        n.href = "/inbox".to_owned();
        actor.stamp(&mut n);
        n
    };
    let processed = item.status == inbox_proto::InboxItem::STATUS_PROCESSED;
    match before {
        None if !processed => Some(InboxChange::Captured(base(ActivityKind::Captured))),
        Some(b) if b.status != inbox_proto::InboxItem::STATUS_PROCESSED && processed => Some(
            InboxChange::Processed(base(ActivityKind::Processed), item.processed_into.clone()),
        ),
        _ => None,
    }
}

/// A processed capture's destination, resolved: `(kind, title, href,
/// project)`.
pub type ProcessedInto = (&'static str, String, String, Option<Uuid>);

/// Resolve `processed_into` against the org's tasks and projects.
/// Blocking (walks the vault) — call from `spawn_blocking`.
#[must_use]
pub fn resolve_processed_into(
    into: &str,
    tasks: &task::TaskBackend,
    projects: &project::ProjectBackend,
) -> ProcessedInto {
    use project::ProjectService as _;
    use task::TaskService as _;
    if let Some(id) = into
        .strip_prefix("project:")
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        let title = projects.get(id).map(|p| p.title).unwrap_or_default();
        return ("project", title, format!("/projects/{id}"), Some(id));
    }
    if let Ok(t) = tasks.get_by_path(into) {
        return ("task", t.title, format!("/tasks/{}", t.id), t.project_id);
    }
    let stem = std::path::Path::new(into)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| into.to_owned());
    ("note", stem, format!("/vault?path={into}"), None)
}

/// Fill a `Processed` entry from its resolved destination.
pub fn apply_processed(n: &mut NewActivity, into: &ProcessedInto) {
    let (kind, title, href, project) = into;
    n.field = (*kind).to_owned();
    n.to.clone_from(title);
    n.href.clone_from(href);
    n.body = (*kind).to_owned();
    if n.project_id.is_none() {
        n.project_id = *project;
    }
}

// ── timer rules ─────────────────────────────────────────────────────

/// The mode a session names, when its description is a steering kind's
/// label ("Maintenance hour") — the server's twin of the app's
/// `now_block::mode_of`.
#[cfg(feature = "plugin-scheduling")]
#[must_use]
pub fn mode_label(description: &str) -> Option<&'static str> {
    scheduling_proto::BlockCategory::from_focus_label(description.trim())
        .and_then(scheduling_proto::BlockCategory::focus_label)
}

/// The steering kinds are the scheduling plugin's; without it no
/// description names one.
#[cfg(not(feature = "plugin-scheduling"))]
#[must_use]
pub fn mode_label(_description: &str) -> Option<&'static str> {
    None
}

fn fmt_duration(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let mins = (to - from).num_minutes().max(0);
    match mins {
        0 => "under a minute".to_owned(),
        m if m < 60 => format!("{m} min"),
        m => format!("{}h {:02}m", m / 60, m % 60),
    }
}

/// A timer session starting (`was_running = false`, now open) or
/// stopping (`was_running = true`, now closed). Sessions logged after
/// the fact, and edits to a running one, say nothing.
// t[impl activity.timer.mode]
#[must_use]
pub fn timer_change(
    was_running: bool,
    s: &timer_proto::WorkSession,
    actor: &Actor,
) -> Option<NewActivity> {
    let running = s.end_time.is_none();
    let mode = mode_label(&s.description);
    let kind = match (was_running, running, mode.is_some()) {
        (false, true, true) => ActivityKind::ModeStarted,
        (false, true, false) => ActivityKind::TimerStarted,
        (true, false, true) => ActivityKind::ModeEnded,
        (true, false, false) => ActivityKind::TimerStopped,
        _ => return None,
    };
    let mut n = NewActivity::new(kind, "timer", s.id);
    n.entity_title = match mode {
        Some(m) => m.to_owned(),
        None if s.description.trim().is_empty() => "a timer".to_owned(),
        None => s.description.clone(),
    };
    n.href = "/timer".to_owned();
    n.project_id = s.project_id;
    if let Some(end) = s.end_time {
        n.body = fmt_duration(s.start_time, end);
    }
    actor.stamp(&mut n);
    Some(n)
}

/// Who ran a timer: the router caller who started/stopped it for that
/// user, else the session's user when the auth store knows them.
#[must_use]
pub fn timer_actor(ledger: &ActorLedger, people: &People, s: &timer_proto::WorkSession) -> Actor {
    let uid = s.user_id.to_string();
    if let Some(u) = ledger.who(&format!("timer-user:{uid}")) {
        return Actor::user(&u, people);
    }
    match people.name(&uid) {
        Some(name) => Actor { id: uid, name },
        None => Actor {
            id: String::new(),
            name: "Someone".to_owned(),
        },
    }
}

// ── project rules ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnap {
    pub title: String,
    pub status: String,
}

// t[impl activity.project]
#[must_use]
pub fn project_change(
    before: Option<&ProjectSnap>,
    p: &project::ProjectInfo,
    actor: &Actor,
) -> Vec<NewActivity> {
    let base = |kind: ActivityKind| {
        let mut n = NewActivity::new(kind, "project", p.id);
        n.entity_title.clone_from(&p.title);
        n.href = format!("/projects/{}", p.id);
        n.project_id = Some(p.id);
        actor.stamp(&mut n);
        n
    };
    let Some(b) = before else {
        return vec![base(ActivityKind::ProjectCreated)];
    };
    let mut out = Vec::new();
    for (field, from, to) in [
        ("title", &b.title, &p.title),
        ("status", &b.status, &p.status),
    ] {
        if from != to {
            let mut n = base(ActivityKind::ProjectUpdated);
            n.field = field.to_owned();
            n.from.clone_from(from);
            n.to.clone_from(to);
            out.push(n);
        }
    }
    out
}

// ── the decorators ──────────────────────────────────────────────────

/// `TaskService` with its writers noted — see the module docs — and
/// its creates attributed to the caller (`crate::attribution`).
#[derive(Clone)]
pub struct AttributedTasks {
    pub inner: task::TaskBackend,
    pub ledger: ActorLedger,
    pub creators: crate::attribution::Creators,
}

impl architect::HasDispatcher for AttributedTasks {
    type Dispatcher = ActorDispatcher;
    fn dispatcher(&self) -> ActorDispatcher {
        ActorDispatcher::attributing(self.creators.clone())
    }
}

impl AttributedTasks {
    fn note(&self, id: Uuid) {
        let key = if id.is_nil() {
            "task:new".to_owned()
        } else {
            format!("task:{id}")
        };
        self.ledger.note(key, caller_user());
    }
}

// t[impl activity.actor]
impl task::TaskService for AttributedTasks {
    fn list(&self) -> Result<Vec<task::TaskInfo>, task::TaskError> {
        self.inner.list()
    }
    fn get(&self, id: Uuid) -> Result<task::TaskInfo, task::TaskError> {
        self.inner.get(id)
    }
    fn get_by_path(&self, path: &str) -> Result<task::TaskInfo, task::TaskError> {
        self.inner.get_by_path(path)
    }
    fn create(&self, mut task: task::TaskInfo) -> Result<task::TaskInfo, task::TaskError> {
        self.note(task.id);
        if !task.id.is_nil() {
            // A client-minted id is the create's key too.
            self.ledger.note("task:new", caller_user());
        }
        // The caller made it — whatever the task it sent says.
        crate::attribution::attribute_task(&mut task, caller_creator().as_ref());
        self.inner.create(task)
    }
    fn update(&self, task: task::TaskInfo) -> Result<task::TaskInfo, task::TaskError> {
        self.note(task.id);
        self.inner.update(task)
    }
    fn try_claim(
        &self,
        id: Uuid,
        agent: String,
        force: bool,
    ) -> Result<task::service::ClaimResult, task::TaskError> {
        self.note(id);
        self.inner.try_claim(id, agent, force)
    }
    fn reverse_relations(&self, id: Uuid) -> Result<Vec<task::ReverseRelation>, task::TaskError> {
        self.inner.reverse_relations(id)
    }
    fn reverse_relations_batch(
        &self,
        ids: Vec<Uuid>,
    ) -> Result<Vec<task::TaskReverseRelations>, task::TaskError> {
        self.inner.reverse_relations_batch(ids)
    }
    fn query(&self, filter: task::TaskListFilter) -> Result<Vec<task::TaskInfo>, task::TaskError> {
        self.inner.query(filter)
    }
    fn rename(&self, id: Uuid, new_path: &str) -> Result<task::TaskInfo, task::TaskError> {
        self.note(id);
        self.inner.rename(id, new_path)
    }
    fn delete(&self, id: Uuid) -> Result<(), task::TaskError> {
        self.note(id);
        self.inner.delete(id)
    }
}

/// `Inbox` with its writers noted, and its captures attributed to the
/// caller (`crate::attribution`).
#[derive(Clone)]
pub struct AttributedInbox {
    pub inner: inbox::VaultInbox,
    pub ledger: ActorLedger,
    pub creators: crate::attribution::Creators,
}

impl architect::HasDispatcher for AttributedInbox {
    type Dispatcher = ActorDispatcher;
    fn dispatcher(&self) -> ActorDispatcher {
        ActorDispatcher::attributing(self.creators.clone())
    }
}

impl inbox_proto::Inbox for AttributedInbox {
    fn list_inbox(&self) -> Result<Vec<inbox_proto::InboxItem>, inbox_proto::InboxError> {
        self.inner.list_inbox()
    }
    fn review_queue(
        &self,
        today: String,
    ) -> Result<Vec<inbox_proto::InboxItem>, inbox_proto::InboxError> {
        self.inner.review_queue(today)
    }
    fn list_inbox_by(
        &self,
        created_by: String,
    ) -> Result<Vec<inbox_proto::InboxItem>, inbox_proto::InboxError> {
        self.inner.list_inbox_by(created_by)
    }
    fn get_inbox_item(&self, id: &str) -> Result<inbox_proto::InboxItem, inbox_proto::InboxError> {
        self.inner.get_inbox_item(id)
    }
    fn upsert_inbox_item(
        &self,
        item: &inbox_proto::InboxItem,
    ) -> Result<(), inbox_proto::InboxError> {
        self.ledger
            .note(format!("inbox:{}", item.id), caller_user());
        // A new capture is the caller's, whatever the item says; one
        // already stored keeps its creator (the backend sees to that).
        let mut item = item.clone();
        crate::attribution::attribute_inbox(&mut item, caller_creator().as_ref());
        self.inner.upsert_inbox_item(&item)
    }
    fn delete_inbox_item(&self, id: &str) -> Result<(), inbox_proto::InboxError> {
        self.ledger.note(format!("inbox:{id}"), caller_user());
        self.inner.delete_inbox_item(id)
    }
    fn due_queue(
        &self,
        today: String,
    ) -> Result<Vec<inbox_proto::InboxItem>, inbox_proto::InboxError> {
        self.inner.due_queue(today)
    }
}

/// `TimerService` with its writers noted, per timer user.
#[derive(Clone)]
pub struct AttributedTimer {
    pub inner: timer::Store,
    pub ledger: ActorLedger,
}

impl AttributedTimer {
    fn note(&self, user: Uuid) {
        self.ledger
            .note(format!("timer-user:{user}"), caller_user());
    }
}

impl timer_proto::TimerService for AttributedTimer {
    async fn start_timer(
        &self,
        req: timer_proto::StartTimerRequest,
    ) -> Result<timer_proto::WorkSession, timer_proto::TimerError> {
        self.note(req.user_id);
        self.inner.start_timer(req).await
    }
    async fn stop_timer(
        &self,
        user_id: Uuid,
    ) -> Result<timer_proto::WorkSession, timer_proto::TimerError> {
        self.note(user_id);
        self.inner.stop_timer(user_id).await
    }
    async fn active_timer(
        &self,
        user_id: Uuid,
    ) -> Result<Option<timer_proto::WorkSession>, timer_proto::TimerError> {
        self.inner.active_timer(user_id).await
    }
    async fn switch_timer(
        &self,
        req: timer_proto::StartTimerRequest,
    ) -> Result<(Option<timer_proto::WorkSession>, timer_proto::WorkSession), timer_proto::TimerError>
    {
        self.note(req.user_id);
        self.inner.switch_timer(req).await
    }
    async fn log_session(
        &self,
        req: timer_proto::LogSessionRequest,
    ) -> Result<timer_proto::WorkSession, timer_proto::TimerError> {
        self.inner.log_session(req).await
    }
    async fn resolve_rate(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<timer_proto::RateResolution, timer_proto::TimerError> {
        self.inner.resolve_rate(user_id, project_id).await
    }
    async fn list_sessions(
        &self,
        filter: timer_proto::WorkSessionFilter,
    ) -> Result<Vec<timer_proto::WorkSession>, timer_proto::TimerError> {
        self.inner.list_sessions(filter).await
    }
    async fn update_session(
        &self,
        req: timer_proto::service::UpdateSessionRequest,
    ) -> Result<timer_proto::WorkSession, timer_proto::TimerError> {
        self.inner.update_session(req).await
    }
    async fn delete_session(&self, id: Uuid) -> Result<(), timer_proto::TimerError> {
        self.inner.delete_session(id).await
    }
    async fn set_org_member_rate(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
        currency: String,
    ) -> Result<timer_proto::OrgMemberRate, timer_proto::TimerError> {
        // Fully qualified: the store has an inherent method of the
        // same name with a different shape.
        timer_proto::TimerService::set_org_member_rate(
            &self.inner,
            org_id,
            user_id,
            hourly_cents,
            currency,
        )
        .await
    }
    async fn set_project_member_rate(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
    ) -> Result<timer_proto::ProjectMemberRate, timer_proto::TimerError> {
        self.inner
            .set_project_member_rate(project_id, user_id, hourly_cents)
            .await
    }
    async fn list_org_member_rates(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<timer_proto::OrgMemberRate>, timer_proto::TimerError> {
        self.inner.list_org_member_rates(org_id).await
    }
    async fn list_project_member_rates(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<timer_proto::ProjectMemberRate>, timer_proto::TimerError> {
        self.inner.list_project_member_rates(project_id).await
    }
    async fn list_tags(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<timer_proto::Tag>, timer_proto::TimerError> {
        self.inner.list_tags(org_id).await
    }
    async fn create_tag(
        &self,
        org_id: Uuid,
        name: String,
        color: String,
    ) -> Result<timer_proto::Tag, timer_proto::TimerError> {
        self.inner.create_tag(org_id, name, color).await
    }
    async fn delete_tag(
        &self,
        org_id: Uuid,
        name: String,
    ) -> Result<timer_proto::Tag, timer_proto::TimerError> {
        self.inner.delete_tag(org_id, name).await
    }
    async fn attach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
    ) -> Result<(), timer_proto::TimerError> {
        self.inner.attach_tags(session_id, org_id, names).await
    }
    async fn detach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
        all: bool,
    ) -> Result<u64, timer_proto::TimerError> {
        self.inner.detach_tags(session_id, org_id, names, all).await
    }
}

/// `ThreadsService` that writes a `commented` entry for every posted
/// message. Threads have no change stream, so this is the one place a
/// comment is seen happening.
#[derive(Clone)]
pub struct RecordingThreads {
    pub inner: threads::Store,
    pub activity: ActivityStore,
    pub tasks: task::TaskBackend,
    pub projects: project::ProjectBackend,
    pub people: People,
}

/// The entry a posted comment makes, anchored where its thread is.
/// `anchor` is `(title, href, project)` for the thread's entity.
// t[impl activity.comment]
#[must_use]
pub fn comment_entry(
    thread: &threads::Thread,
    msg: &threads::Message,
    anchor: (String, String, Option<Uuid>),
    actor: &Actor,
) -> NewActivity {
    let (title, href, project) = anchor;
    let mut n = NewActivity::new(
        ActivityKind::Commented,
        &thread.entity_type,
        thread.entity_id,
    );
    n.at = Some(msg.posted_at);
    n.entity_title = title;
    n.href = href;
    n.project_id = project;
    n.body = if msg.body.chars().count() > 1000 {
        msg.body.chars().take(1000).collect()
    } else {
        msg.body.clone()
    };
    actor.stamp(&mut n);
    n
}

impl RecordingThreads {
    /// `(title, href, project)` for a thread's anchor entity.
    async fn anchor(&self, entity_type: &str, id: Uuid) -> (String, String, Option<Uuid>) {
        match entity_type {
            "task" => {
                let tasks = self.tasks.clone();
                let t = tokio::task::spawn_blocking(move || {
                    use task::TaskService as _;
                    tasks.get(id).ok()
                })
                .await
                .ok()
                .flatten();
                (
                    t.as_ref().map(|t| t.title.clone()).unwrap_or_default(),
                    format!("/tasks/{id}"),
                    t.and_then(|t| t.project_id),
                )
            }
            "project" => {
                let projects = self.projects.clone();
                let p = tokio::task::spawn_blocking(move || {
                    use project::ProjectService as _;
                    projects.get(id).ok()
                })
                .await
                .ok()
                .flatten();
                (
                    p.map(|p| p.title).unwrap_or_default(),
                    format!("/projects/{id}"),
                    Some(id),
                )
            }
            _ => (String::new(), String::new(), None),
        }
    }
}

impl threads::ThreadsService for RecordingThreads {
    async fn list_threads(
        &self,
        entity_type: String,
        entity_id: Uuid,
    ) -> Result<Vec<threads::Thread>, threads::ThreadsError> {
        self.inner.list_threads(entity_type, entity_id).await
    }
    async fn get_thread(&self, id: Uuid) -> Result<threads::Thread, threads::ThreadsError> {
        self.inner.get_thread(id).await
    }
    async fn create_thread(
        &self,
        req: threads::CreateThreadRequest,
    ) -> Result<threads::Thread, threads::ThreadsError> {
        self.inner.create_thread(req).await
    }
    async fn list_messages(
        &self,
        thread_id: Uuid,
    ) -> Result<Vec<threads::Message>, threads::ThreadsError> {
        self.inner.list_messages(thread_id).await
    }
    async fn post_message(
        &self,
        req: threads::PostMessageRequest,
    ) -> Result<threads::Message, threads::ThreadsError> {
        let msg = self.inner.post_message(req).await?;
        // The comment stands whatever happens to its history line.
        if let Ok(thread) = self.inner.get_thread(msg.thread_id).await {
            let anchor = self.anchor(&thread.entity_type, thread.entity_id).await;
            let caller = caller_user();
            let id = msg
                .author_id
                .map(|u| u.to_string())
                .or(caller)
                .unwrap_or_default();
            let name = Some(msg.author_label.clone())
                .filter(|l| !l.trim().is_empty())
                .or_else(|| self.people.name(&id))
                .unwrap_or_else(|| "Someone".to_owned());
            let entry = comment_entry(&thread, &msg, anchor, &Actor { id, name });
            match self.activity.record(entry).await {
                Ok(a) => architect_telemetry::wide::set("activity.kind", a.kind.as_str()),
                Err(e) => {
                    architect_telemetry::wide::set("activity.error", e.to_string());
                }
            }
        }
        Ok(msg)
    }
    async fn set_resolved(
        &self,
        thread_id: Uuid,
        resolved: bool,
        by: Option<Uuid>,
    ) -> Result<threads::Thread, threads::ThreadsError> {
        self.inner.set_resolved(thread_id, resolved, by).await
    }
    async fn delete_thread(&self, id: Uuid) -> Result<(), threads::ThreadsError> {
        self.inner.delete_thread(id).await
    }
    async fn delete_message(&self, id: Uuid) -> Result<(), threads::ThreadsError> {
        self.inner.delete_message(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn someone() -> Actor {
        Actor {
            id: "u1".into(),
            name: "Alice".into(),
        }
    }

    fn task_with(status: &str) -> task::TaskInfo {
        let mut t = task::TaskInfo::new("Comp lead vocals");
        t.id = Uuid::new_v4();
        t.status = status.into();
        t
    }

    // t[verify activity.task.diff]
    #[test]
    fn a_task_change_says_what_changed() {
        let people = People::default();
        let mut t = task_with("open");
        let before = TaskSnap::of(&t, &people);

        // First sight after the seed is a creation.
        let created = task_changes(None, &t, &before, &someone(), None);
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].kind, Some(ActivityKind::Created));
        assert_eq!(created[0].href, format!("/tasks/{}", t.id));

        // Status, priority, due and assignee, one entry each, from → to.
        t.status = "in-progress".into();
        t.priority = "high".into();
        t.due = Some("2026-10-20".into());
        t.workflow = Some(task::model::WorkflowAttrs {
            assignees: task::model::AgentRefList(vec![task::workflows_proto::AgentRef::Human {
                user_id: "u2".into(),
                display_name: Some("Sam".into()),
            }]),
            ..Default::default()
        });
        let after = TaskSnap::of(&t, &people);
        let out = task_changes(Some(&before), &t, &after, &someone(), None);
        let kinds: Vec<_> = out.iter().filter_map(|n| n.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActivityKind::StatusChanged,
                ActivityKind::Assigned,
                ActivityKind::PriorityChanged,
                ActivityKind::DueChanged,
            ]
        );
        let status = &out[0];
        assert_eq!(
            (status.from.as_str(), status.to.as_str()),
            ("open", "in-progress")
        );
        assert_eq!(out[1].to, "Sam");
        assert!(out.iter().all(|n| n.actor_name == "Alice"));

        // Into a terminal state is a completion; out of one, a reopen.
        let mut done = t.clone();
        done.status = "done".into();
        let snap_done = TaskSnap::of(&done, &people);
        let out = task_changes(Some(&after), &done, &snap_done, &someone(), None);
        assert_eq!(out[0].kind, Some(ActivityKind::Completed));
        let out = task_changes(Some(&snap_done), &t, &after, &someone(), None);
        assert_eq!(out[0].kind, Some(ActivityKind::Reopened));

        // Nothing changed, nothing said.
        assert!(task_changes(Some(&after), &t, &after, &someone(), None).is_empty());
    }

    #[test]
    fn description_edits_coalesce_into_one_entry_per_burst() {
        let people = People::default();
        let mut t = task_with("open");
        let before = TaskSnap::of(&t, &people);
        t.details = "first draft".into();
        let after = TaskSnap::of(&t, &people);
        let out = task_changes(Some(&before), &t, &after, &someone(), None);
        assert_eq!(out[0].kind, Some(ActivityKind::Edited));
        // Saved again a moment later: quiet.
        let out = task_changes(Some(&before), &t, &after, &someone(), Some(Instant::now()));
        assert!(out.is_empty());
    }

    #[test]
    fn renaming_names_the_task_by_its_new_title() {
        let people = People::default();
        let mut t = task_with("open");
        let before = TaskSnap::of(&t, &people);
        t.title = "Comp lead vocals — Track One".into();
        let after = TaskSnap::of(&t, &people);
        let out = task_changes(Some(&before), &t, &after, &someone(), None);
        assert_eq!(out[0].kind, Some(ActivityKind::Renamed));
        assert_eq!(out[0].entity_title, "Comp lead vocals — Track One");
        assert_eq!(out[0].from, "Comp lead vocals");
    }

    // t[verify activity.actor]
    #[test]
    fn the_ledger_names_the_router_caller_and_forgets() {
        let ledger = ActorLedger::default();
        let people = People::default();
        people.insert("u9", "Sam");
        let id = Uuid::new_v4();
        let snap = TaskSnap::of(&task_with("open"), &people);
        // No caller recorded: nobody (the task has no assignee).
        assert_eq!(
            task_actor(&ledger, &people, id, Some(&snap)).name,
            "Someone"
        );
        ledger.note(format!("task:{id}"), Some("u9".into()));
        let a = task_actor(&ledger, &people, id, Some(&snap));
        assert_eq!((a.id.as_str(), a.name.as_str()), ("u9", "Sam"));
        // A create keyed "task:new" covers an id the backend mints.
        let other = Uuid::new_v4();
        ledger.note("task:new", Some("u9".into()));
        assert_eq!(task_actor(&ledger, &people, other, None).name, "Sam");
        // No user, nothing noted.
        ledger.note("inbox:x", None);
        assert!(ledger.who("inbox:x").is_none());
    }

    // t[verify activity.inbox]
    #[test]
    fn captures_and_their_processing() {
        let mut item = inbox_proto::InboxItem::capture(
            "c1",
            "Call the piano tuner\n\nabout Thursday",
            "ui",
            "2026-10-12T09:00:00Z".to_owned(),
        );
        let Some(InboxChange::Captured(n)) = inbox_change(None, &item, &someone()) else {
            panic!("a new capture is captured");
        };
        assert_eq!(n.entity_title, "Call the piano tuner");
        let open = InboxSnap {
            status: item.status.clone(),
        };
        item.status = inbox_proto::InboxItem::STATUS_PROCESSED.into();
        item.processed_into = Some("project:00000000-0000-0000-0000-000000000001".into());
        let Some(InboxChange::Processed(mut n, into)) =
            inbox_change(Some(&open), &item, &someone())
        else {
            panic!("open → processed is processed");
        };
        assert_eq!(into.as_deref(), item.processed_into.as_deref());
        apply_processed(
            &mut n,
            &(
                "project",
                "Bonus edition".into(),
                "/projects/1".into(),
                None,
            ),
        );
        assert_eq!(
            (n.to.as_str(), n.href.as_str()),
            ("Bonus edition", "/projects/1")
        );
        // Re-saving a processed capture says nothing more.
        let done = InboxSnap {
            status: item.status.clone(),
        };
        assert!(inbox_change(Some(&done), &item, &someone()).is_none());
    }

    // t[verify activity.timer.mode]
    #[test]
    fn a_maintenance_hour_is_a_mode_and_a_plain_timer_is_a_timer() {
        let mut s = timer_proto::WorkSession {
            id: Uuid::new_v4(),
            org_id: Uuid::nil(),
            user_id: Uuid::nil(),
            project_id: None,
            project_path: String::new(),
            description: "Maintenance hour".into(),
            start_time: DateTime::from_timestamp(1_800_000_000, 0).expect("ts"),
            end_time: None,
            billable: false,
            rate_cents: 0,
            currency: String::new(),
            task_note_path: String::new(),
            invoice_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let n = timer_change(false, &s, &someone()).expect("started");
        assert_eq!(n.kind, Some(ActivityKind::ModeStarted));
        assert_eq!(n.entity_title, "Maintenance hour");
        // Edits to a running session: quiet.
        assert!(timer_change(true, &s, &someone()).is_none());
        s.end_time = Some(s.start_time + chrono::Duration::minutes(47));
        let n = timer_change(true, &s, &someone()).expect("ended");
        assert_eq!(n.kind, Some(ActivityKind::ModeEnded));
        assert_eq!(n.body, "47 min");
        // A session logged after the fact: quiet.
        assert!(timer_change(false, &s, &someone()).is_none());

        s.description = "Mixing".into();
        s.end_time = None;
        assert_eq!(
            timer_change(false, &s, &someone()).and_then(|n| n.kind),
            Some(ActivityKind::TimerStarted)
        );
    }

    #[test]
    fn a_project_change_names_the_field() {
        let mut p = project::ProjectInfo {
            id: Uuid::new_v4(),
            title: "Example Album".into(),
            status: "active".into(),
            ..Default::default()
        };
        let created = project_change(None, &p, &someone());
        assert_eq!(created[0].kind, Some(ActivityKind::ProjectCreated));
        assert_eq!(created[0].project_id, Some(p.id));
        let before = ProjectSnap {
            title: p.title.clone(),
            status: p.status.clone(),
        };
        p.status = "completed".into();
        let out = project_change(Some(&before), &p, &someone());
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].field.as_str(), out[0].to.as_str()),
            ("status", "completed")
        );
    }
}
