//! Signed grants for the colocated-media route.
//!
//! `GET /org/{slug}/media/{path}` used to be open to anyone; it now takes
//! a short-lived Ed25519-signed `?token=` minted over vox (see
//! `MediaService::media_grant` and `apps/task/server/tests/media_auth.rs`).
//! Every URL this crate builds has the same shape —
//! `/org/{org}/media/songs/{slug}/…` — so one grant per (org, song)
//! covers a song's whole load: manifest, chart, and every stem.
//!
//! ## Why the grant is fetched separately from the URL
//!
//! `HTMLMediaElement::set_src` is synchronous, so a stem URL cannot await
//! a mint. The load paths here are async and already touch the network
//! before any `set_src`, so they warm the cache with [`suffix`] and the
//! synchronous sites read it back with [`cached_suffix`].
//!
//! ## Failure is a no-op, on purpose
//!
//! A failed mint (offline, or a server predating `media_grant`) yields an
//! EMPTY suffix, i.e. exactly the URL this crate built before. While
//! `TASK_ENFORCE_MEDIA_TOKEN` is off that still plays, so the rollout
//! order is: ship this, watch the wide events show `auth_via=signed-token`,
//! and only then enforce. A hard failure here would have made the client
//! half a flag-day with the server half.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Cached grants, keyed by `(org, media prefix)` — the prefix the
/// grant was minted over, e.g. `songs/going-home` or `projects`.
/// Value is `(query suffix, expires_unix)`.
type Grants = HashMap<(String, String), (String, i64)>;

fn grants() -> &'static Mutex<Grants> {
    static GRANTS: OnceLock<Mutex<Grants>> = OnceLock::new();
    GRANTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Seconds of headroom before expiry at which we re-mint rather than
/// hand back a token that may die mid-song.
const REFRESH_MARGIN_SECONDS: i64 = 5 * 60;

fn now_unix() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        // `Date::now()` is ms since epoch; no `SystemTime` on wasm.
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default()
    }
}

/// The prefix a song's media lives under. One grant over this covers
/// the song's whole load — manifest, chart, and every stem.
fn song_prefix(slug: &str) -> String {
    format!("songs/{slug}")
}

/// Look up a cached, still-valid suffix without minting.
fn cached(org: &str, prefix: &str) -> Option<String> {
    let key = (org.to_owned(), prefix.to_owned());
    let g = grants().lock().ok()?;
    let (suffix, expires) = g.get(&key)?;
    (*expires - REFRESH_MARGIN_SECONDS > now_unix()).then(|| suffix.clone())
}

/// The query suffix for any `/org/{org}/media/{prefix}/…` URL —
/// `?token=…`, or `""` when no grant could be obtained.
///
/// The prefix is the caller's, not this crate's: songs are the busiest
/// consumer but not the only one — project covers live under
/// `projects/`, and a grant minted over one prefix must not be handed
/// to a URL under another, or the server rejects it.
///
/// Mints on a miss and caches for the grant's lifetime; concurrent
/// callers may briefly mint twice, which is harmless (grants are
/// stateless and the later one simply replaces the earlier).
pub async fn suffix_for_prefix(org: &str, prefix: &str) -> String {
    if let Some(hit) = cached(org, prefix) {
        return hit;
    }
    let Some(grant) = mint(org, prefix).await else {
        return String::new();
    };
    let suffix = format!("?token={}", grant.token);
    if let Ok(mut g) = grants().lock() {
        g.insert(
            (org.to_owned(), prefix.to_owned()),
            (suffix.clone(), grant.expires_unix),
        );
    }
    suffix
}

/// [`suffix_for_prefix`] over a song's own prefix.
pub async fn suffix(org: &str, slug: &str) -> String {
    suffix_for_prefix(org, &song_prefix(slug)).await
}

/// The suffix for a grant already minted by an earlier
/// [`suffix_for_prefix`] call — for synchronous sites
/// (`<audio>.set_src`) that cannot await.
///
/// Empty when nothing is cached, which yields the pre-change URL.
#[must_use]
pub fn cached_suffix_for_prefix(org: &str, prefix: &str) -> String {
    cached(org, prefix).unwrap_or_default()
}

/// [`cached_suffix_for_prefix`] over a song's own prefix.
#[must_use]
pub fn cached_suffix(org: &str, slug: &str) -> String {
    cached_suffix_for_prefix(org, &song_prefix(slug))
}

async fn mint(org: &str, prefix: &str) -> Option<media_proto::MediaGrant> {
    use crate::vox_clients::establish_for;
    use media_proto::MediaServiceClient;

    let client: MediaServiceClient = establish_for(org).await.ok()?;
    match client.media_grant(prefix.to_owned()).await {
        Ok(grant) => Some(grant),
        Err(e) => {
            // Not fatal: without a grant the URL is what it always was,
            // which still serves until the server flag is flipped.
            tracing::warn!(org, prefix, "media grant unavailable: {e:?}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_grant_yields_the_pre_change_url() {
        // The rollout contract: no grant → empty suffix → the exact URL
        // this crate built before signing existed.
        assert_eq!(cached_suffix("nobody", "nothing"), "");
    }

    #[test]
    fn an_expired_grant_is_not_reused() {
        grants().lock().unwrap().insert(
            ("org".into(), song_prefix("song")),
            ("?token=stale".into(), now_unix() - 1),
        );
        assert_eq!(cached_suffix("org", "song"), "");
    }

    #[test]
    fn a_grant_inside_the_refresh_margin_is_not_reused() {
        // Handing back a token about to expire would strand a stem
        // mid-song; re-mint instead.
        grants().lock().unwrap().insert(
            ("org".into(), song_prefix("soon")),
            (
                "?token=soon".into(),
                now_unix() + REFRESH_MARGIN_SECONDS / 2,
            ),
        );
        assert_eq!(cached_suffix("org", "soon"), "");
    }

    #[test]
    fn a_fresh_grant_is_reused() {
        grants().lock().unwrap().insert(
            ("org".into(), song_prefix("fresh")),
            ("?token=good".into(), now_unix() + 3600),
        );
        assert_eq!(cached_suffix("org", "fresh"), "?token=good");
    }

    #[test]
    fn prefixes_do_not_share_a_grant() {
        // A token minted over `songs/x` is invalid for `projects` — the
        // server checks the prefix, so the cache must not conflate two
        // prefixes that happen to share an org.
        grants().lock().unwrap().insert(
            ("org".into(), "projects".into()),
            ("?token=covers".into(), now_unix() + 3600),
        );
        assert_eq!(cached_suffix_for_prefix("org", "projects"), "?token=covers");
        assert_eq!(cached_suffix("org", "projects"), "");
    }
}
