//! One-org RPC calls, and the fan-out that runs them across many orgs.
//!
//! The Task UI's data layer is the same six lines over and over:
//! establish one org's service client, call one method, normalize the
//! failure to `"{slug}: <what>: {e:?}"`. This module holds the
//! machinery — the [`feeds!`] declaration macro and the multi-org
//! [`fan_out`] helpers — so the shell and every feature UI crate write
//! their calls the same way, and a feature crate's protos stay in the
//! feature crate.

// [`feeds!`] declares those calls, grouped by the service client so the
// client type is named once per service instead of once per call.
//
// ```ignore
// feeds! {
//     inbox_proto::InboxClient {
//         /// Doc comment lands on the generated function.
//         fetch_inbox() -> Vec<inbox_proto::InboxItem>
//             = list_inbox() as "list inbox";
//
//         /// Extra parameters follow `slug: &str`; the argument list is
//         /// the RPC's, so it can use them.
//         delete_inbox_item(id: &str) -> ()
//             = delete_inbox_item(id.to_string()) as "delete inbox item";
//     }
// }
// ```
//
// Two optional pieces:
//
// - `map <expr>,` before `as` inserts a `.map(<expr>)` between the call
//   and the error mapping — for RPCs that return `()` where the caller
//   wants the value it sent back.
// - `as` takes any `Display` expression, so a message that needs an
//   argument is `as format!("day plan {date}")`.
//
// Anything that does more than this — building a request struct,
// post-processing rows — stays a hand-written `pub async fn` beside the
// macro invocation. Multi-org views go through [`fan_out`] /
// [`fan_out_tagged`] below.

/// Declare per-org feed calls. See the module-level shape notes above.
///
/// Exported at the crate root: `use task_ui_core::feeds;` then
/// `feeds! { … }`.
#[macro_export]
macro_rules! feeds {
    ($(
        $client:ty {
            $(
                $(#[$meta:meta])*
                $name:ident($($arg:ident: $aty:ty),* $(,)?) -> $ret:ty
                    = $method:ident($($marg:expr),* $(,)?)
                    $(map $map:expr,)?
                    as $what:expr;
            )*
        }
    )*) => {
        $($(
            $(#[$meta])*
            pub async fn $name(slug: &str $(, $arg: $aty)*) -> Result<$ret, String> {
                let client = $crate::vox_clients::establish_for::<$client>(slug).await?;
                client
                    .$method($($marg),*)
                    .await
                    $(.map($map))?
                    .map_err(|e| format!("{}: {}: {:?}", slug, $what, e))
            }
        )*)*
    };
}

// ── single-flight: one RPC per (call, org set), not one per consumer ──
//
// A store-backed list hook (`use_multi_org_list` → architect's
// `use_store_list`) owns its OWN `use_resource`, so every mounted
// component that reads the same shared store issues the same fetch:
// the shell, the command palette and the page each asked for
// `project/list`, and each of those fanned out across every selected
// org. Measured against the deployed server that was ~30 identical
// `project/list` + ~28 `task/list` per reconnect — and vox counts a
// live request against `max_concurrent_requests` (64 by default,
// `r[rpc.flow-control.max-concurrent-requests.counting]`), with the
// ~11 long-lived `*-stream/events` subscriptions already holding
// slots. Crossing 64 is not backpressure: the server closes the
// connection as a protocol violation, the client reconnects, re-runs
// every resource, and trips the limit again — a reconnect storm that
// cannot converge.
//
// So concurrent callers that want the SAME rows await the SAME call,
// exactly as `vox_clients::shared_caller_with` does for dials. This is
// deliberately NOT a result cache: the entry lives only while the call
// is in flight, so a later fetch still goes to the server and
// `Store::reload` still means reload.
//
// `fan_out` only coalesces on wasm: the web app is where the fan-out
// multiplies (one connection per org, every store live at once) and
// where the storm was observed, and wasm being single-threaded lets the
// registry be a `thread_local` + `LocalBoxFuture` without forcing a
// `Send` bound onto every caller's future. Native keeps the direct call.
//
// The module itself compiles on BOTH targets — the native half is what
// the tests at the bottom of this file exercise, so the coalescing
// contract is covered by `cargo nextest` rather than only by the wasm
// build nobody runs tests against.
//
// (Native therefore sees the module as unused outside `cfg(test)`.)
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod single_flight {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;

    use futures_util::FutureExt;
    #[cfg(not(target_arch = "wasm32"))]
    use futures_util::future::BoxFuture;
    #[cfg(target_arch = "wasm32")]
    use futures_util::future::LocalBoxFuture;
    use futures_util::future::Shared;

    /// What makes two fan-outs "the same call": the client type, the
    /// row type, whether rows come back slug-tagged, the operation
    /// label, and the exact org set.
    pub type Key = (TypeId, &'static str, String, String);

    #[cfg(target_arch = "wasm32")]
    type Flight<R> = Shared<LocalBoxFuture<'static, R>>;
    #[cfg(not(target_arch = "wasm32"))]
    type Flight<R> = Shared<BoxFuture<'static, R>>;

    #[cfg(target_arch = "wasm32")]
    fn with_flights<T>(f: impl FnOnce(&mut HashMap<Key, Box<dyn Any>>) -> T) -> T {
        use std::cell::RefCell;
        thread_local! {
            static IN_FLIGHT: RefCell<HashMap<Key, Box<dyn Any>>> =
                RefCell::new(HashMap::new());
        }
        IN_FLIGHT.with(|m| f(&mut m.borrow_mut()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_flights<T>(f: impl FnOnce(&mut HashMap<Key, Box<dyn Any + Send>>) -> T) -> T {
        use std::sync::{Mutex, OnceLock};
        static IN_FLIGHT: OnceLock<Mutex<HashMap<Key, Box<dyn Any + Send>>>> = OnceLock::new();
        let m = IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()));
        // Poisoning only means some caller panicked mid-map-edit; the
        // map is still a valid registry, so recover rather than cascade.
        let mut guard = m.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Await the in-flight call for `key`, or start it and let later
    /// callers join. The lock is never held across the await.
    #[cfg(target_arch = "wasm32")]
    pub async fn dedupe<R, Fut>(key: Key, make: impl FnOnce() -> Fut) -> R
    where
        R: Clone + 'static,
        Fut: std::future::Future<Output = R> + 'static,
    {
        let existing = with_flights(|m| {
            m.get(&key)
                .and_then(|f| f.downcast_ref::<Flight<R>>().cloned())
        });
        if let Some(flight) = existing {
            return flight.await;
        }
        let flight: Flight<R> = (Box::pin(make()) as LocalBoxFuture<'static, R>).shared();
        with_flights(|m| m.insert(key.clone(), Box::new(flight.clone())));
        let out = flight.await;
        with_flights(|m| m.remove(&key));
        out
    }

    /// Native twin of the wasm [`dedupe`] — same contract, `Send`
    /// futures. Not on `fan_out`'s path (see the module note); it exists
    /// so the coalescing contract is testable.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn dedupe<R, Fut>(key: Key, make: impl FnOnce() -> Fut) -> R
    where
        R: Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
    {
        // The key carries a `TypeId` that includes the row type, so a
        // hit is always the right `Flight<R>`. `downcast_ref` is the
        // belt to that braces — a miss starts a fresh call rather than
        // handing back the wrong rows.
        let existing = with_flights(|m| {
            m.get(&key)
                .and_then(|f| f.downcast_ref::<Flight<R>>().cloned())
        });
        if let Some(flight) = existing {
            return flight.await;
        }
        let flight: Flight<R> = (Box::pin(make()) as BoxFuture<'static, R>).shared();
        with_flights(|m| m.insert(key.clone(), Box::new(flight.clone())));
        let out = flight.await;
        // Clear the slot so the NEXT fetch really hits the server.
        // Callers already awaiting this `Shared` keep its result.
        with_flights(|m| m.remove(&key));
        out
    }
}

/// Fan one org-scoped list call out across `slugs`, concatenating the
/// rows. Per-org failures are tolerated (a down or empty org doesn't
/// blank the whole view); an error surfaces only if *nothing* came back.
///
/// Concurrent callers asking for the same rows share one call — see
/// [`single_flight`] for why that matters on the wire.
pub async fn fan_out<C, T, E, F, Fut>(
    slugs: &[String],
    what: &str,
    call: F,
) -> Result<Vec<T>, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
    T: Clone + 'static,
    F: Fn(C) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<Vec<T>, E>> + 'static,
    E: std::fmt::Debug + 'static,
{
    let slugs = slugs.to_vec();
    #[cfg(target_arch = "wasm32")]
    {
        let key = (
            std::any::TypeId::of::<(C, T)>(),
            "plain",
            what.to_owned(),
            slugs.join(","),
        );
        let what = what.to_owned();
        single_flight::dedupe(key, move || fan_out_inner::<C, T, E, F, Fut>(slugs, what, call))
            .await
    }
    #[cfg(not(target_arch = "wasm32"))]
    fan_out_inner::<C, T, E, F, Fut>(slugs, what.to_owned(), call).await
}

async fn fan_out_inner<C, T, E, F, Fut>(
    slugs: Vec<String>,
    what: String,
    call: F,
) -> Result<Vec<T>, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
    F: Fn(C) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, E>>,
    E: std::fmt::Debug,
{
    let futs = slugs.iter().map(|slug| {
        let call = &call;
        let what = &what;
        async move {
            match crate::vox_clients::establish_for::<C>(slug).await {
                Ok(client) => call(client)
                    .await
                    .map_err(|e| format!("{slug}: {what}: {e:?}")),
                Err(e) => Err(format!("{slug}: {e}")),
            }
        }
    });
    collect(futures_util::future::join_all(futs).await)
}

/// [`fan_out`], pairing every row with the slug of the org it came from
/// — so mutations and detail pages can route back to the owning org.
pub async fn fan_out_tagged<C, T, E, F, Fut>(
    slugs: &[String],
    what: &str,
    call: F,
) -> Result<Vec<(String, T)>, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
    T: Clone + 'static,
    F: Fn(C) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<Vec<T>, E>> + 'static,
    E: std::fmt::Debug + 'static,
{
    let slugs = slugs.to_vec();
    #[cfg(target_arch = "wasm32")]
    {
        let key = (
            std::any::TypeId::of::<(C, T)>(),
            "tagged",
            what.to_owned(),
            slugs.join(","),
        );
        let what = what.to_owned();
        single_flight::dedupe(key, move || {
            fan_out_tagged_inner::<C, T, E, F, Fut>(slugs, what, call)
        })
        .await
    }
    #[cfg(not(target_arch = "wasm32"))]
    fan_out_tagged_inner::<C, T, E, F, Fut>(slugs, what.to_owned(), call).await
}

async fn fan_out_tagged_inner<C, T, E, F, Fut>(
    slugs: Vec<String>,
    what: String,
    call: F,
) -> Result<Vec<(String, T)>, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
    F: Fn(C) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, E>>,
    E: std::fmt::Debug,
{
    let futs = slugs.iter().map(|slug| {
        let call = &call;
        let what = &what;
        async move {
            match crate::vox_clients::establish_for::<C>(slug).await {
                Ok(client) => call(client)
                    .await
                    .map(|rows| {
                        rows.into_iter()
                            .map(|r| (slug.clone(), r))
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| format!("{slug}: {what}: {e:?}")),
                Err(e) => Err(format!("{slug}: {e}")),
            }
        }
    });
    collect(futures_util::future::join_all(futs).await)
}

/// Flatten per-org results: concat the successes; surface an error only
/// if every org failed *and* nothing came back.
pub fn collect<T>(results: Vec<Result<Vec<T>, String>>) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    let mut last_err = None;
    for r in results {
        match r {
            Ok(rows) => out.extend(rows),
            Err(e) => last_err = Some(e),
        }
    }
    if out.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(out)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::single_flight::{Key, dedupe};

    /// Distinct `what` per test: the registry is process-global and
    /// `cargo nextest` runs tests concurrently.
    fn key(what: &str) -> Key {
        (
            std::any::TypeId::of::<u32>(),
            "test",
            what.to_owned(),
            "acme,vnt".to_owned(),
        )
    }

    /// The whole point: N consumers of one shared store asked for the
    /// same rows at once and issued N identical RPCs. They must now
    /// issue ONE.
    #[test]
    fn overlapping_identical_calls_issue_one_request() {
        let runs = Arc::new(AtomicUsize::new(0));
        // The call has to still be in flight when the second caller
        // arrives — that is the case coalescing exists for, and an
        // already-ready future would not exercise it.
        let (tx, rx) = futures_channel::oneshot::channel::<u32>();

        let first = {
            let runs = Arc::clone(&runs);
            dedupe(key("overlap"), move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                rx.await.unwrap_or(0)
            })
        };
        let second = {
            let runs = Arc::clone(&runs);
            dedupe(key("overlap"), move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                0u32
            })
        };
        // `join` polls in order: `first` registers the flight and parks
        // on the channel, `second` joins it, then the sender resolves
        // both.
        let (a, b, _) = pollster::block_on(futures_util::future::join3(first, second, async {
            let _ = tx.send(7);
        }));

        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "expected ONE underlying call"
        );
        assert_eq!((a, b), (7, 7), "both callers get the shared result");
    }

    /// Different orgs / operations are different calls — coalescing
    /// must not collapse them into one.
    #[test]
    fn distinct_keys_do_not_share() {
        let runs = Arc::new(AtomicUsize::new(0));
        let mk = |k: Key| {
            let runs = Arc::clone(&runs);
            dedupe(k, move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                1u32
            })
        };
        pollster::block_on(futures_util::future::join(
            mk(key("distinct-a")),
            mk(key("distinct-b")),
        ));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// A registry, not a cache: once a call finishes, the next fetch
    /// must really hit the server — otherwise `Store::reload` would
    /// silently stop reloading.
    #[test]
    fn a_later_call_is_not_served_from_the_registry() {
        let runs = Arc::new(AtomicUsize::new(0));
        let once = || {
            let runs = Arc::clone(&runs);
            pollster::block_on(dedupe(key("sequential"), move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                1u32
            }))
        };
        once();
        once();
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }
}
