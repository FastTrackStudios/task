//! The store machinery — the pattern, without any domain in it.
//!
//! Task's optimistic stores were one 2,200-line module in the shell:
//! the generic shape and twenty-six domains' worth of entities, in the
//! same file. That was fine while every page lived in the shell too,
//! and became the thing blocking apps the moment pages started moving
//! out — a plugin crate cannot depend on `task-ui`, so it could not
//! reach the pattern its own page was written against.
//!
//! So the pattern lives here and the domains live with their features.
//! This crate names no proto crate and no entity; it knows about orgs,
//! because the fan-out is genuinely part of the shape (Task reads and
//! writes are slug-routed, and a store under "All organizations" holds
//! rows from several at once), and about nothing else.
//!
//! What a feature crate does with it:
//!
//! ```ignore
//! task_stores::stores! {
//!     BodyMetricStore: BodyMetric {
//!         provide: provide_body_metric_store,
//!         handle: use_body_metric_store,
//!         list: use_body_metric_list -> Uuid = fetch_body_metrics,
//!         mutations: BodyMetricMutations via use_body_metric_mutations,
//!     }
//! }
//! ```
//!
//! That generates the crate's own `provide_stores()`, which the app
//! root has to call. A feature the shell mounts directly is called
//! from the shell; a plugin declares `provide` on its `PluginApp` and
//! the shell calls it for every registered app.

use dioxus::prelude::*;
use task_ui_core::orgs::{OrgMeta, OrgSelection};

pub use architect::{
    AtomResult, Id, Mutation, Store, StoreEntity, use_mutation, use_store, use_store_entry,
    use_store_list,
};
/// Re-exported so a `stores!` invocation needs no dioxus import of its
/// own for the hooks the expansion calls.
pub use dioxus::prelude::{use_context, use_context_provider};

#[macro_export]
macro_rules! stores {
    ($(
        $(#[$smeta:meta])*
        $alias:ident: $entity:ty {
            provide: $provide:ident,
            handle: $handle:ident,
            $(
                list:
                $(#[$lmeta:meta])*
                $list:ident -> $key:ty = $fetch:path,
            )?
            $(
                stream:
                $(#[$stmeta:meta])*
                $sscope:ident $sclient:ty => $sfold:path,
            )?
            $(
                mutations:
                $(#[$mmeta:meta])*
                $muts:ident via $usemuts:ident,
            )?
        }
    )*) => {
        /// Provide every feature store at the app root (after
        /// `architect::use_app_supervised`, which provides the notifications +
        /// reactivity registries the mutations report into).
        pub fn provide_stores() {
            $($provide();)*
        }

        $(
            $(#[$smeta])*
            pub type $alias = $crate::Store<$entity, String>;

            #[doc = concat!("Install the shared [`", stringify!($alias), "`] at the app root.")]
            pub fn $provide() -> $alias {
                let store = $crate::use_store();
                let store = $crate::use_context_provider(move || store);
                $(
                    // Doc lines on the `stream:` table row ($stmeta)
                    // document the table; nothing to attach them to
                    // in expansion (statement doc-comments warn).
                    $crate::use_org_event_streams(
                        store,
                        $crate::stream_scope::$sscope(),
                        |slug: String, tx| async move {
                            let Ok(client) = task_ui_core::vox_clients::establish_for::<$sclient>(
                                &slug,
                            )
                            .await
                            else {
                                return false;
                            };
                            client.events(tx).await.is_ok()
                        },
                        $sfold,
                    );
                )?
                store
            }

            #[doc = concat!("Handle to the app-root [`", stringify!($alias), "`].")]
            pub fn $handle() -> $alias {
                $crate::use_context()
            }

            $(
                $(#[$lmeta])*
                pub fn $list() -> $crate::AtomResult<Vec<($crate::Id<$key>, $entity)>, String> {
                    $crate::use_first_org_list($handle(), |slug| async move { $fetch(&slug).await })
                }
            )?

            $(
                $(#[$mmeta])*
                #[derive(Clone, Copy)]
                pub struct $muts {
                    store: $alias,
                    write: $crate::Mutation<String>,
                }

                #[doc = concat!("Handle for [`", stringify!($muts), "`].")]
                pub fn $usemuts() -> $muts {
                    $muts { store: $handle(), write: $crate::use_mutation() }
                }
            )?
        )*
    };
}

pub fn use_org_scope() -> (Signal<OrgSelection>, Signal<Vec<OrgMeta>>) {
    (use_context(), use_context())
}

// ── live store streams (fetch-once-then-fold, per org) ──────────────

/// Which orgs a store's live subscription attaches to — mirrors the
/// fetch scope of its list hook, so events only ever fold into a
/// store that holds (or will hold) that org's rows.
#[derive(Clone, Copy, PartialEq)]
pub enum StreamScope {
    /// Every selected org (multi-org, slug-tagged stores).
    All,
    /// Just the first selected org (single-org register stores —
    /// their list hook fetches only the first selected slug, so an
    /// event from any other org would desync the cache).
    First,
}

/// Lowercase constructors so the [`stores!`] table reads
/// `stream: all …` / `stream: first …`.
pub mod stream_scope {
    #[must_use]
    pub fn all() -> super::StreamScope {
        super::StreamScope::All
    }
    #[must_use]
    pub fn first() -> super::StreamScope {
        super::StreamScope::First
    }
}

/// Live-store subscription driver: one healing event pump per
/// selected org, folding every received event into the shared store.
///
/// The generic behind the [`stores!`] `stream:` line — the store
/// version of `architect::use_stream`, extended for Task's multi-org
/// fan-out (a store under "All" holds rows from several orgs, so it
/// needs one subscription *per org*, and the fold must know which
/// org's stream an event came from — the `slug` parameter).
///
/// Semantics (mirroring the `/vault` + `/wiki` page consumers):
/// - `subscribe(slug, tx)` establishes that org's stream client and
///   holds the subscribe call in flight — the future staying pending
///   *is* the healthy state; it resolving means the subscription
///   ended (`false` = never established).
/// - an ended subscription is retried after a backoff (~400 ms when
///   it had been live, 1s→8s doubling when it never established);
/// - every *re*-subscribe first re-runs the store's backing fetch
///   ([`Store::reload`] — the `subscribed_once` recovery pattern):
///   the hubs are sliding mailboxes, nothing is replayed, so events
///   published while detached are recovered by refetching. No-op
///   until a list hook has mounted, which is also correct — a page
///   that mounts later starts with its own fresh fetch.
/// - changing the org selection re-runs the whole hook (the closure
///   reads the selection signals), dropping every pump and
///   subscribing against the new slug set. The list hooks refetch on
///   the same signal change, so no reload is needed for that case.
pub fn use_org_event_streams<T, Ev, F, Fut>(
    store: Store<T, String>,
    scope: StreamScope,
    subscribe: F,
    fold: fn(&Store<T, String>, &str, Ev),
) where
    T: StoreEntity,
    Ev: Clone + vox::facet::Facet<'static> + 'static,
    F: Fn(String, vox::Tx<Ev>) -> Fut + Clone + 'static,
    Fut: std::future::Future<Output = bool> + 'static,
{
    let (selection, orgs) = use_org_scope();
    use_resource(move || {
        let mut slugs = task_ui_core::orgs::selected_slugs(&selection.read(), &orgs.read());
        if scope == StreamScope::First {
            slugs.truncate(1);
        }
        let subscribe = subscribe.clone();
        async move {
            // One endless, self-healing pump per selected org. The
            // future never resolves; it is dropped (cancelling every
            // in-flight subscribe) when the selection changes or the
            // app unmounts.
            let pumps: Vec<_> = slugs
                .into_iter()
                .map(|slug| pump_org_events(store, slug, subscribe.clone(), fold))
                .collect();
            futures_util::future::join_all(pumps).await;
        }
    });
}

/// One org's endless subscribe → pump → backoff → resubscribe loop.
/// See [`use_org_event_streams`] for the recovery contract.
async fn pump_org_events<T, Ev, F, Fut>(
    store: Store<T, String>,
    slug: String,
    subscribe: F,
    fold: fn(&Store<T, String>, &str, Ev),
) where
    T: StoreEntity,
    Ev: Clone + vox::facet::Facet<'static> + 'static,
    F: Fn(String, vox::Tx<Ev>) -> Fut + Clone + 'static,
    Fut: std::future::Future<Output = bool> + 'static,
{
    let mut first_attempt = true;
    // Consecutive failures to establish — the backoff's only state.
    let mut misses = 0u32;
    loop {
        if !first_attempt {
            // Re-subscribing after a gap: refetch so events published
            // while we were detached (sliding hubs replay nothing)
            // are recovered. Same restart-then-resubscribe ordering
            // the `/vault` page uses.
            store.reload();
        }
        first_attempt = false;

        let (tx, mut rx) = vox::channel::<Ev>();
        let call = subscribe(slug.clone(), tx);
        let pump = async {
            while let Ok(Some(event)) = rx.recv().await {
                // `SelfRef` has no owned extraction; `map` lends the
                // value while the receive buffer is alive, so cloning
                // out is sound — events own their data.
                let mut owned: Option<Ev> = None;
                let _ = event.map(|ev| owned = Some(ev.clone()));
                if let Some(ev) = owned {
                    fold(&store, &slug, ev);
                }
            }
        };
        // Race the in-flight subscribe call against the pump: the
        // call resolving means the subscription is over (couldn't
        // establish, server error, or server-side stream end).
        let mut call = core::pin::pin!(call);
        let mut pump = core::pin::pin!(pump);
        let outcome = core::future::poll_fn(move |cx| {
            use core::future::Future;
            use core::task::Poll;
            if let Poll::Ready(established) = call.as_mut().poll(cx) {
                return Poll::Ready(Some(established));
            }
            pump.as_mut().poll(cx).map(|()| None)
        })
        .await;

        // A pump that ended had events flowing — treat it as a live
        // stream that stopped rather than one that never started.
        let was_live = !matches!(outcome, Some(false));
        let wait_ms = if was_live {
            misses = 0;
            400
        } else {
            let n = misses;
            misses = (n + 1).min(4);
            1000u64 << n.min(3)
        };
        architect::platform::sleep(core::time::Duration::from_millis(wait_ms)).await;
    }
}

// ── event folds ─────────────────────────────────────────────────────
//
// One per live store: the server's fetch-once-then-fold subscriber
// contract, aimed at the shared optimistic cache. `Upserted` events
// carry the full post-write record → `Store::put` (idempotent);
// `Deleted` carries the key → `Store::remove_real`. Multi-org stores
// re-tag the row with the slug of the stream it arrived on.

pub fn use_first_org_list<T, F, Fut>(
    store: Store<T, String>,
    fetch: F,
) -> AtomResult<Vec<(Id<T::Key>, T)>, String>
where
    T: StoreEntity,
    F: Fn(String) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<Vec<T>, String>> + 'static,
{
    let (selection, orgs) = use_org_scope();
    use_store_list(store, move || {
        let slug = task_ui_core::orgs::selected_slugs(&selection.read(), &orgs.read())
            .into_iter()
            .next();
        let pending = slug.map(&fetch);
        async move { Some(pending?.await) }
    })
}

/// Store-backed list fanned out over **every selected org** — the shape
/// of the multi-org views (tasks, projects, sessions, invoices). An
/// empty slug set (discovery pending) keeps the phase at `Loading`.
#[allow(clippy::type_complexity)] // `AtomResult<Vec<(Id, T)>, _>` reads fine.
pub fn use_multi_org_list<T, F, Fut>(
    store: Store<T, String>,
    fetch: F,
) -> AtomResult<Vec<(Id<T::Key>, T)>, String>
where
    T: StoreEntity,
    F: Fn(Vec<String>) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<Vec<T>, String>> + 'static,
{
    let (selection, orgs) = use_org_scope();
    use_store_list(store, move || {
        let slugs = task_ui_core::orgs::selected_slugs(&selection.read(), &orgs.read());
        let pending = (!slugs.is_empty()).then(|| fetch(slugs));
        async move { Some(pending?.await) }
    })
}

/// The optimistic-create lifecycle every feature shares: insert the
/// draft now, swap it for the server's row on success, roll back (and
/// notify) on failure.
pub fn run_create<T, Fut>(
    write: Mutation<String>,
    store: Store<T, String>,
    draft: T,
    call: impl FnOnce(T) -> Fut + 'static,
) where
    T: StoreEntity,
    Fut: std::future::Future<Output = Result<T, String>> + 'static,
{
    let send = draft.clone();
    write.run(
        store,
        move |s| s.insert_optimistic(draft).0,
        move || async move { call(send).await.map(Some) },
    );
}
