//! The notifier — materializes [`notify_proto::Notification`]s from
//! the org's event hubs, by rules.
//!
//! One instance per hosted org, spawned from [`AppState::new`]. It
//! subscribes to the org's `#[subscribe]`
//! streams over an **in-process** [`architect::LocalServer`] — no
//! socket, no TCP; a raw `vox::channel` Tx attached straight to a
//! `PubSub` never drains (nothing resolves its sink), so the local
//! transport is the sanctioned in-process consumption path — and
//! delivers each rule hit through every configured
//! [`notify::DeliveryChannel`]:
//!
//! - [`notify::InApp`] — persists into the org's `notify.sqlite` and
//!   publishes on the `Notify` events stream (the bell).
//! - [`notify::Webhook`] — when `TASK_NOTIFY_WEBHOOK` is set.
//!
//! # The rule catalog
//!
//! | rule | hub | fires when |
//! |---|---|---|
//! | [`task_rule`] | task | status transitions into terminal (`TaskCompleted`); the primary assignee changes (`TaskAssigned`) |
//! | [`agent_rule`] | agent | a turn finishes (`AgentTurnFinished`) or errors (`AgentTurnFailed`) — routine runs surface as turns on their routine session |
//! | [`booking_rule`] | scheduling | a booking id first appears (`BookingCreated`); a known booking flips to `Cancelled` (`BookingCancelled`) |
//! | [`forge_rule`] | git issues + reviews | an issue opens/closes (`ForgeIssue`); a PR opens or is reviewed (`ForgePullRequest`) |
//!
//! Each rule is a few lines returning `Option<NewNotification>` —
//! adding one is: pick the hub, add a `NotifyKind` variant, write the
//! match arm. Transition-shaped rules (task status/assignee, booking
//! status) keep a small snapshot cache seeded from the backend at
//! (re)subscribe time, because the hubs replay nothing.
//!
//! Self-noise: where an event names its actor (a claim's assignee),
//! the actor lands on the notification so surfaces can suppress or
//! byline it; org-level notifications have no per-recipient fan-out
//! yet, so that is the whole debounce story today.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_proto::event::{AgentEvent, AgentEventEnvelope};
use git_proto::GitEvent;
use notify::{DeliveryChannel, InApp, NewNotification, Store, Webhook};
use notify_proto::{NotifyKind, NotifySource};
use scheduling_proto::{Booking, BookingStatus, SchedulingEvent};
use task::model::TaskInfo;
use task::service::TaskEvent;
use uuid::Uuid;

use crate::{AppState, OrgAppState};

/// The in-flight subscribe call a pump races against its drain loop —
/// boxed so pumps whose subscribe arms build different client futures
/// (the two forge hubs) share one type.
type SubCall = std::pin::Pin<Box<dyn Future<Output = bool> + Send>>;

/// Spawn one notifier per hosted org. Called once at boot from
/// [`AppState::new`]; an org hot-added later (`create_org`) gets its
/// notifier on the next boot — same lifecycle as the forge poll loop.
pub fn spawn(state: &AppState) {
    for slug in state.org_slugs() {
        if let Some(org) = state.org(&slug) {
            spawn_org(org, Arc::clone(&state.scope));
        }
    }
}

/// Wire one org: build the channel list, serve the org router over an
/// in-process link, and start one pump per subscribed hub. The pumps
/// live until `scope` closes (their LocalServer acceptors are
/// deferred on it).
fn spawn_org(org: OrgAppState, scope: Arc<architect::Scope>) {
    let deliver = Arc::new(Deliverer::new(&org));
    let local = Arc::new(architect::LocalServer::serve(
        crate::org_layer_router(&org),
        scope,
    ));

    // Task rule — always mounted (core plugin).
    {
        let cache: Arc<Mutex<HashMap<Uuid, TaskSnapshot>>> = Arc::default();
        let subscribe = {
            let local = Arc::clone(&local);
            let tasks = org.tasks.clone();
            let cache = Arc::clone(&cache);
            move || {
                let local = Arc::clone(&local);
                let tasks = tasks.clone();
                let cache = Arc::clone(&cache);
                async move {
                    // Subscribe FIRST, then seed (the fetch-once-then-
                    // fold contract): events landing during the seed
                    // wait in the rx mailbox and replay over a cache
                    // that already contains their effect — idempotent.
                    // Seeding re-runs on every resubscribe because the
                    // hub replays nothing, and unknown-id events fire
                    // nothing — the seed is what arms the rule.
                    let client: task::TaskServiceStreamClient = local.establish().await.ok()?;
                    let (tx, rx) = vox::channel::<TaskEvent>();
                    let call = tokio::spawn(async move { client.events(tx).await.is_ok() });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let seeded = tokio::task::spawn_blocking(move || {
                        task::service::TaskService::list(&tasks)
                    })
                    .await;
                    match seeded {
                        Ok(Ok(rows)) => {
                            *cache.lock().expect("task cache lock") =
                                rows.iter().map(|t| (t.id, TaskSnapshot::of(t))).collect();
                        }
                        Ok(Err(e)) => tracing::warn!(error = ?e, "notifier: task seed failed"),
                        Err(e) => tracing::warn!(error = %e, "notifier: task seed panicked"),
                    }
                    let call: SubCall = Box::pin(async move { call.await.unwrap_or(false) });
                    Some((call, rx))
                }
            }
        };
        let deliver = Arc::clone(&deliver);
        tokio::spawn(run_pump("task", subscribe, move |ev| {
            if let Some(n) = task_rule(&mut cache.lock().expect("task cache lock"), ev) {
                deliver.deliver(n);
            }
        }));
    }

    // Email rule — new mail in any configured mailbox.
    #[cfg(feature = "plugin-email")]
    if org.plugins.contains("email") {
        spawn_email(org.clone(), Arc::clone(&deliver));
    }

    // Agent rule — only when the agent plugin is mounted.
    if org.plugins.contains("agent") {
        let subscribe = {
            let local = Arc::clone(&local);
            move || {
                let local = Arc::clone(&local);
                async move {
                    let client: agent_proto::service::subscriptions::SubscriptionsStreamClient =
                        local.establish().await.ok()?;
                    let (tx, rx) = vox::channel::<AgentEventEnvelope>();
                    let call: SubCall = Box::pin(async move { client.events(tx).await.is_ok() });
                    Some((call, rx))
                }
            }
        };
        let deliver = Arc::clone(&deliver);
        tokio::spawn(run_pump("agent", subscribe, move |ev| {
            if let Some(n) = agent_rule(&ev) {
                deliver.deliver(n);
            }
        }));
    }

    // Booking rule — only when the scheduling plugin is mounted.
    if org.plugins.contains("scheduling") {
        let cache: Arc<Mutex<HashMap<String, BookingStatus>>> = Arc::default();
        let subscribe = {
            let local = Arc::clone(&local);
            let scheduler = org.scheduling.clone();
            let cache = Arc::clone(&cache);
            move || {
                let local = Arc::clone(&local);
                let scheduler = scheduler.clone();
                let cache = Arc::clone(&cache);
                async move {
                    // Subscribe first, then seed — see the task pump.
                    // The seed keeps an edit to a pre-existing booking
                    // from reading as "new booking".
                    let client: scheduling_proto::SchedulingEventsStreamClient =
                        local.establish().await.ok()?;
                    let (tx, rx) = vox::channel::<SchedulingEvent>();
                    let call = tokio::spawn(async move { client.events(tx).await.is_ok() });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let seeded = tokio::task::spawn_blocking(move || {
                        scheduling_proto::service::bookings::Bookings::list_bookings(&scheduler)
                    })
                    .await;
                    match seeded {
                        Ok(Ok(rows)) => {
                            *cache.lock().expect("booking cache lock") = rows
                                .into_iter()
                                .map(|b| (b.id.0.clone(), b.status))
                                .collect();
                        }
                        Ok(Err(e)) => tracing::warn!(error = ?e, "notifier: booking seed failed"),
                        Err(e) => tracing::warn!(error = %e, "notifier: booking seed panicked"),
                    }
                    let call: SubCall = Box::pin(async move { call.await.unwrap_or(false) });
                    Some((call, rx))
                }
            }
        };
        let deliver = Arc::clone(&deliver);
        tokio::spawn(run_pump("scheduling", subscribe, move |ev| {
            if let Some(n) = booking_rule(&mut cache.lock().expect("booking cache lock"), ev) {
                deliver.deliver(n);
            }
        }));
    }

    // Forge rules — only when the forge plugin is mounted. Two hubs,
    // one rule fn: both streams carry `GitEvent`.
    if org.plugins.contains("git") {
        for (what, issues) in [("forge-issues", true), ("forge-reviews", false)] {
            let subscribe = {
                let local = Arc::clone(&local);
                move || {
                    let local = Arc::clone(&local);
                    async move {
                        let (tx, rx) = vox::channel::<GitEvent>();
                        let call: SubCall = if issues {
                            let client: git_proto::issues::IssueTrackerStreamClient =
                                local.establish().await.ok()?;
                            Box::pin(async move { client.issue_events(tx).await.is_ok() })
                        } else {
                            let client: git_proto::reviews::ReviewSurfaceStreamClient =
                                local.establish().await.ok()?;
                            Box::pin(async move { client.review_events(tx).await.is_ok() })
                        };
                        Some((call, rx))
                    }
                }
            };
            let deliver = Arc::clone(&deliver);
            tokio::spawn(run_pump(what, subscribe, move |ev| {
                if let Some(n) = forge_rule(&ev) {
                    deliver.deliver(n);
                }
            }));
        }
    }
}

// ── delivery ────────────────────────────────────────────────────────

/// The org's configured channel fan-out. One mint per rule hit, so
/// every channel reports the same notification identity.
/// New-mail notifications.
///
/// Unlike the other rules this is a **poll, not a stream**. The whole
/// alert-once pipeline already exists in `email-store`: the
/// `email-product` background pass fetches recent envelopes, baselines
/// on first sight of an account, and marks genuinely-new messages
/// unnotified. Nothing consumed the other end — `unnotified()` was
/// written and never read, so mail arrived and no notification ever
/// fired.
///
/// Baselining is why this must go through the store rather than off the
/// `NewMessage` event: connecting a mailbox with a few hundred existing
/// messages would otherwise fire a few hundred notifications. First
/// sight records everything as already-seen and fires nothing; only
/// mail that shows up *after* that raises anything.
///
/// The ids are drained, turned into notifications, and marked — in that
/// order. A crash between deliver and mark re-notifies, which is the
/// right way round: a duplicate is noise, a silent drop is a missed
/// email.
#[cfg(feature = "plugin-email")]
fn spawn_email(org: OrgAppState, deliver: Arc<Deliverer>) {
    use email_proto::{EmailProduct, EmailSync, SeqRange};

    /// Matches `email-product`'s own poller, so a new message waits at
    /// most one product pass plus one of these.
    const POLL: Duration = Duration::from_secs(30);
    /// Per-pass ceiling. `notify_observe` already caps what it marks;
    /// this is the second belt.
    const BATCH: u32 = 20;

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL).await;

            let accounts = {
                let email = org.email.clone();
                match tokio::task::spawn_blocking(move || email.accounts()).await {
                    Ok(Ok(list)) => list,
                    Ok(Err(err)) => {
                        tracing::debug!(?err, "notifier: email accounts unavailable");
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(%err, "notifier: email accounts panicked");
                        continue;
                    }
                }
            };

            for account in accounts {
                let id = account.id.0.clone();
                let product = org.email_product.clone();
                let acct = id.clone();
                let pending =
                    match tokio::task::spawn_blocking(move || product.unnotified(&acct, BATCH))
                        .await
                    {
                        Ok(Ok(ids)) if !ids.is_empty() => ids,
                        // An account with no store (or a transient backend
                        // error) is not worth logging every 30s.
                        _ => continue,
                    };

                // Ids alone make a useless notification ("new message:
                // <opaque@id>"), so pull the headers we already have
                // cheaply. One listing covers the whole batch, and on
                // IMAP it also warms the UID index.
                let email = org.email.clone();
                let acct = id.clone();
                let envelopes = tokio::task::spawn_blocking(move || {
                    email.fetch_envelopes(&acct, "INBOX", SeqRange::Recent(50))
                })
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();

                let mut delivered = Vec::new();
                for message_id in pending {
                    let env = envelopes.iter().find(|e| e.message_id == message_id);
                    // A message that has aged out of the recent window
                    // between the product pass and now still deserves a
                    // notification — just a plainer one.
                    let (title, body) = match env {
                        Some(e) => {
                            let from = e
                                .from
                                .first()
                                .map(|a| {
                                    a.name
                                        .clone()
                                        .filter(|n| !n.is_empty())
                                        .unwrap_or_else(|| a.email.clone())
                                })
                                .unwrap_or_else(|| "unknown sender".to_owned());
                            let subject = if e.subject.is_empty() {
                                "(no subject)".to_owned()
                            } else {
                                e.subject.clone()
                            };
                            (subject, format!("{from} · {}", account.address))
                        }
                        None => ("New mail".to_owned(), account.address.clone()),
                    };
                    deliver.deliver(NewNotification {
                        kind: NotifyKind::EmailReceived,
                        title,
                        body,
                        source: NotifySource {
                            service: "email".to_owned(),
                            entity: message_id.clone(),
                            href: "/email".to_owned(),
                        },
                        actor: account.address.clone(),
                    });
                    delivered.push(message_id);
                }

                if !delivered.is_empty() {
                    let product = org.email_product.clone();
                    let acct = id.clone();
                    let n = delivered.len();
                    match tokio::task::spawn_blocking(move || {
                        product.mark_notified(&acct, delivered)
                    })
                    .await
                    {
                        Ok(Ok(_)) => tracing::debug!(account = %id, n, "notified new mail"),
                        // Not marking means we re-notify next pass.
                        // Loud, because it is a duplicate-notification
                        // loop until it clears.
                        Ok(Err(err)) => tracing::warn!(account = %id, ?err, "mark_notified failed"),
                        Err(err) => tracing::warn!(account = %id, %err, "mark_notified panicked"),
                    }
                }
            }
        }
    });
}

struct Deliverer {
    slug: String,
    channels: Vec<Arc<dyn DeliveryChannel>>,
}

impl Deliverer {
    fn new(org: &OrgAppState) -> Self {
        let mut channels: Vec<Arc<dyn DeliveryChannel>> =
            vec![Arc::new(InApp::new(org.notify.clone()))];
        if let Some(webhook) = Webhook::from_env() {
            channels.push(Arc::new(webhook));
        }
        Self {
            slug: org.slug.clone(),
            channels,
        }
    }

    fn deliver(&self, new: NewNotification) {
        let row = Store::mint(new);
        tracing::debug!(org = %self.slug, kind = row.kind.as_str(), title = %row.title, "notification");
        for channel in &self.channels {
            channel.deliver(&self.slug, &row);
        }
    }
}

// ── the pump ────────────────────────────────────────────────────────

/// One hub's endless subscribe → drain → backoff → resubscribe loop
/// (the server-side twin of the UI stores' event pump). `subscribe`
/// re-seeds any rule cache, establishes the stream client, and returns
/// the in-flight subscribe call plus the receiving end; returning
/// `None` means "couldn't establish" (backed off and retried).
async fn run_pump<Ev, S, Fut, H>(what: &'static str, subscribe: S, mut on_event: H)
where
    Ev: vox::facet::Facet<'static> + Clone + 'static,
    S: Fn() -> Fut,
    Fut: Future<Output = Option<(SubCall, vox::Rx<Ev>)>>,
    H: FnMut(Ev),
{
    let mut misses = 0u32;
    loop {
        let Some((call, mut rx)) = subscribe().await else {
            misses = (misses + 1).min(4);
            tokio::time::sleep(Duration::from_secs(1u64 << misses.min(3))).await;
            continue;
        };
        misses = 0;
        let drain = async {
            while let Ok(Some(msg)) = rx.recv().await {
                // `SelfRef` has no owned extraction; `map` lends the
                // value, so cloning out is sound — events own their
                // data.
                let mut owned: Option<Ev> = None;
                let _ = msg.map(|ev| owned = Some(ev.clone()));
                if let Some(ev) = owned {
                    on_event(ev);
                }
            }
        };
        // Race the in-flight subscribe call against the drain: the
        // call resolving means the subscription ended.
        tokio::select! {
            established = call => {
                if !established {
                    tracing::warn!(hub = what, "notifier: subscribe refused");
                }
            }
            () = drain => {}
        }
        tracing::debug!(hub = what, "notifier: stream ended; resubscribing");
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

// ── rules ───────────────────────────────────────────────────────────

/// What the task rule remembers per task — enough to see status and
/// assignee *transitions* in full-state `Upserted` payloads.
struct TaskSnapshot {
    terminal: bool,
    assignee: Option<String>,
}

impl TaskSnapshot {
    fn of(t: &TaskInfo) -> Self {
        Self {
            terminal: task::status_is_terminal(&t.status),
            assignee: t
                .workflow
                .as_ref()
                .and_then(|w| w.assignees.0.first())
                .map(|a| a.short_label()),
        }
    }
}

/// Task completed (status → terminal) and task claimed/assigned (the
/// primary assignee changed — `try_claim` publishes the claimed task).
/// Events for tasks the cache has never seen are recorded but fire
/// nothing: without a before-state there is no transition, and the
/// cache is seeded from `list()` at subscribe time.
fn task_rule(cache: &mut HashMap<Uuid, TaskSnapshot>, ev: TaskEvent) -> Option<NewNotification> {
    match ev {
        TaskEvent::Upserted(t) => {
            let now = TaskSnapshot::of(&t);
            let before = cache.insert(t.id, TaskSnapshot::of(&t))?;
            let source = || NotifySource {
                service: "task".into(),
                entity: t.id.to_string(),
                href: format!("/tasks/{}", t.id),
            };
            if !before.terminal && now.terminal {
                return Some(NewNotification {
                    kind: NotifyKind::TaskCompleted,
                    title: format!("Task done: {}", t.title),
                    body: format!("Status: {}", t.status),
                    source: source(),
                    actor: now.assignee.unwrap_or_default(),
                });
            }
            if let Some(assignee) = &now.assignee {
                if before.assignee.as_ref() != Some(assignee) {
                    return Some(NewNotification {
                        kind: NotifyKind::TaskAssigned,
                        title: format!("Task claimed: {}", t.title),
                        body: format!("Assigned to {assignee}"),
                        source: source(),
                        actor: assignee.clone(),
                    });
                }
            }
            None
        }
        TaskEvent::Deleted(id) => {
            cache.remove(&id);
            None
        }
    }
}

/// Agent turn finished / errored. A routine run is a turn on its
/// routine's session, so it lands here too. The actor is the agent —
/// exactly the "tell me when it's done" case.
fn agent_rule(env: &AgentEventEnvelope) -> Option<NewNotification> {
    let source = |session: &str| NotifySource {
        service: "agent".into(),
        entity: session.to_owned(),
        href: format!("/agents?session={session}"),
    };
    match &env.event {
        AgentEvent::TurnFinished { session_id, .. } => Some(NewNotification {
            kind: NotifyKind::AgentTurnFinished,
            title: "Agent turn finished".into(),
            body: format!("Session {session_id}"),
            source: source(session_id),
            actor: "agent".into(),
        }),
        AgentEvent::TurnErrored {
            session_id,
            kind,
            message,
            ..
        } => Some(NewNotification {
            kind: NotifyKind::AgentTurnFailed,
            title: "Agent turn failed".into(),
            body: format!("{kind}: {message}"),
            source: source(session_id),
            actor: "agent".into(),
        }),
        _ => None,
    }
}

/// Booking landed / cancelled. First sight of an id (post-seed) is a
/// creation; a known id flipping to `Cancelled` is a cancellation;
/// every other status change (confirm, complete, no-show) is quiet.
fn booking_rule(
    cache: &mut HashMap<String, BookingStatus>,
    ev: SchedulingEvent,
) -> Option<NewNotification> {
    let SchedulingEvent::BookingUpserted(b) = ev else {
        return None;
    };
    let before = cache.insert(b.id.0.clone(), b.status);
    let source = |b: &Booking| NotifySource {
        service: "scheduling".into(),
        entity: b.id.0.clone(),
        href: "/bookings".into(),
    };
    match before {
        None if b.status != BookingStatus::Cancelled => Some(NewNotification {
            kind: NotifyKind::BookingCreated,
            title: format!("New booking: {}", b.attendee_name),
            body: format!("{} — {}", b.start_utc, b.attendee_email),
            source: source(&b),
            actor: b.attendee_name.clone(),
        }),
        Some(was) if was != BookingStatus::Cancelled && b.status == BookingStatus::Cancelled => {
            Some(NewNotification {
                kind: NotifyKind::BookingCancelled,
                title: format!("Booking cancelled: {}", b.attendee_name),
                body: format!("{} — {}", b.start_utc, b.attendee_email),
                source: source(&b),
                actor: b.attendee_name.clone(),
            })
        }
        _ => None,
    }
}

/// Forge news: issues opening/closing, PRs opening / being reviewed.
/// `GitEvent` carries ids only (no assignee/reviewer payload), so
/// "assigned to you" / "review requested" granularity waits on richer
/// events; these fire on the writes this server commits plus whatever
/// the poll loop publishes.
fn forge_rule(ev: &GitEvent) -> Option<NewNotification> {
    let source = |repo: &git_proto::RepoId, entity: String| NotifySource {
        service: "forge".into(),
        entity: format!("{}/{}#{entity}", repo.owner, repo.repo),
        href: "/repos".into(),
    };
    match ev {
        GitEvent::IssueCreated { repo, issue } => Some(NewNotification {
            kind: NotifyKind::ForgeIssue,
            title: format!("Issue #{} opened in {}/{}", issue.0, repo.owner, repo.repo),
            body: String::new(),
            source: source(repo, issue.0.to_string()),
            actor: String::new(),
        }),
        GitEvent::IssueUpdated { repo, issue, state }
            if *state == git_proto::IssueState::Closed =>
        {
            Some(NewNotification {
                kind: NotifyKind::ForgeIssue,
                title: format!("Issue #{} closed in {}/{}", issue.0, repo.owner, repo.repo),
                body: String::new(),
                source: source(repo, issue.0.to_string()),
                actor: String::new(),
            })
        }
        GitEvent::PullRequestCreated { repo, pr } => Some(NewNotification {
            kind: NotifyKind::ForgePullRequest,
            title: format!("PR #{} opened in {}/{}", pr.0, repo.owner, repo.repo),
            body: String::new(),
            source: source(repo, pr.0.to_string()),
            actor: String::new(),
        }),
        GitEvent::PullRequestReviewed { repo, pr } => Some(NewNotification {
            kind: NotifyKind::ForgePullRequest,
            title: format!("PR #{} reviewed in {}/{}", pr.0, repo.owner, repo.repo),
            body: String::new(),
            source: source(repo, pr.0.to_string()),
            actor: String::new(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: Uuid, status: &str, assignee: Option<&str>) -> TaskInfo {
        let mut t = task::capture("ship it");
        t.id = id;
        t.status = status.into();
        if let Some(a) = assignee {
            t.workflow = Some(task::model::WorkflowAttrs {
                assignees: task::model::AgentRefList(vec![task::workflows_proto::AgentRef::agent(
                    a,
                )]),
                ..Default::default()
            });
        }
        t
    }

    #[test]
    fn task_rule_fires_on_transitions_only() {
        let mut cache = HashMap::new();
        let id = Uuid::new_v4();

        // Unknown id: recorded, no notification (no before-state).
        assert!(task_rule(&mut cache, TaskEvent::Upserted(task(id, "done", None))).is_none());
        // Already terminal → still terminal: quiet.
        assert!(task_rule(&mut cache, TaskEvent::Upserted(task(id, "done", None))).is_none());

        let id2 = Uuid::new_v4();
        assert!(task_rule(&mut cache, TaskEvent::Upserted(task(id2, "open", None))).is_none());
        // open → done: fires.
        let n = task_rule(&mut cache, TaskEvent::Upserted(task(id2, "done", None)))
            .expect("completion");
        assert_eq!(n.kind, NotifyKind::TaskCompleted);
        assert_eq!(n.source.href, format!("/tasks/{id2}"));
        // Re-publishing the done state: quiet (no re-notify).
        assert!(task_rule(&mut cache, TaskEvent::Upserted(task(id2, "done", None))).is_none());

        // Claim: assignee appears.
        let n = task_rule(
            &mut cache,
            TaskEvent::Upserted(task(id2, "done", Some("triage-bot"))),
        )
        .expect("claim");
        assert_eq!(n.kind, NotifyKind::TaskAssigned);
        assert_eq!(n.actor, "triage-bot");

        // Deleted clears the snapshot.
        assert!(task_rule(&mut cache, TaskEvent::Deleted(id2)).is_none());
        assert!(!cache.contains_key(&id2));
    }

    #[test]
    fn booking_rule_first_sight_and_cancellation() {
        let mut cache = HashMap::new();
        let b = |status: BookingStatus| scheduling_proto::Booking {
            path: "scheduling/bookings/b1.md".into(),
            id: scheduling_proto::BookingId("b1".into()),
            event_type_id: scheduling_proto::EventTypeId("c30".into()),
            start_utc: "2026-08-01T09:00:00+00:00".into(),
            end_utc: "2026-08-01T09:30:00+00:00".into(),
            attendee_name: "Alice".into(),
            attendee_email: "alice@example.com".into(),
            note: None,
            status,
            created_utc: "2026-07-27T00:00:00+00:00".into(),
        };
        let n = booking_rule(
            &mut cache,
            SchedulingEvent::BookingUpserted(b(BookingStatus::Pending)),
        )
        .expect("created");
        assert_eq!(n.kind, NotifyKind::BookingCreated);
        // Confirmation: quiet.
        assert!(
            booking_rule(
                &mut cache,
                SchedulingEvent::BookingUpserted(b(BookingStatus::Confirmed)),
            )
            .is_none()
        );
        let n = booking_rule(
            &mut cache,
            SchedulingEvent::BookingUpserted(b(BookingStatus::Cancelled)),
        )
        .expect("cancelled");
        assert_eq!(n.kind, NotifyKind::BookingCancelled);
        // Cancelled again: quiet.
        assert!(
            booking_rule(
                &mut cache,
                SchedulingEvent::BookingUpserted(b(BookingStatus::Cancelled)),
            )
            .is_none()
        );
    }

    #[test]
    fn agent_rule_turn_endings_only() {
        let fin = AgentEventEnvelope {
            session_id: "s1".into(),
            event: AgentEvent::TurnFinished {
                session_id: "s1".into(),
                message_id: "m1".into(),
                at: chrono::Utc::now(),
            },
        };
        let n = agent_rule(&fin).expect("finished");
        assert_eq!(n.kind, NotifyKind::AgentTurnFinished);
        assert_eq!(n.source.href, "/agents?session=s1");

        let started = AgentEventEnvelope {
            session_id: "s1".into(),
            event: AgentEvent::TurnStarted {
                session_id: "s1".into(),
                stream_id: "st".into(),
                at: chrono::Utc::now(),
            },
        };
        assert!(agent_rule(&started).is_none());
    }
}
