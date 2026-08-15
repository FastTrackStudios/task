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

/// Fan one org-scoped list call out across `slugs`, concatenating the
/// rows. Per-org failures are tolerated (a down or empty org doesn't
/// blank the whole view); an error surfaces only if *nothing* came back.
pub async fn fan_out<C, T, E, F, Fut>(slugs: &[String], what: &str, call: F) -> Result<Vec<T>, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
    F: Fn(C) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, E>>,
    E: std::fmt::Debug,
{
    let futs = slugs.iter().map(|slug| {
        let call = &call;
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
    F: Fn(C) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, E>>,
    E: std::fmt::Debug,
{
    let futs = slugs.iter().map(|slug| {
        let call = &call;
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
