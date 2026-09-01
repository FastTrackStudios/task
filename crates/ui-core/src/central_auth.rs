//! Where this server says accounts come from.
//!
//! A Task server can delegate identity to a central issuer
//! (`fts-auth`), so one account spans every FastTrackStudio app instead
//! of one account per org. The server advertises the issuer in
//! `/.well-known/task-server.json`; this is where the client keeps it.
//!
//! # Why a registry and not a signal
//!
//! Discovery parses on a code path with no reach into the app's signal
//! graph, and sign-in needs the answer from a plain async function
//! rather than a component. That is the same shape as each org's iroh
//! endpoint id, solved the same way and for the same reason — see
//! `iroh_transport::note_org_endpoints`.
//!
//! # Absent is the normal case
//!
//! A self-hosted server advertises nothing here, and then everything
//! behaves as it always has: sign in against the home org, whose
//! `auth.sqlite` is the authority. Central auth is what you opt into,
//! not what you opt out of.

use std::sync::RwLock;

static ISSUER: RwLock<Option<String>> = RwLock::new(None);

/// Record what discovery said. `None` clears it — a server that stops
/// advertising an issuer has taken identity back, and a stale value
/// would send sign-in somewhere the server no longer trusts.
pub fn note(issuer: Option<String>) {
    let cleaned = issuer
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty());
    if let Ok(mut slot) = ISSUER.write() {
        if *slot != cleaned {
            match &cleaned {
                Some(url) => tracing::info!(issuer = %url, "identity: central issuer"),
                None => tracing::info!("identity: this server issues its own accounts"),
            }
        }
        *slot = cleaned;
    }
}

/// The configured issuer's base URL, if this server has one.
#[must_use]
pub fn issuer() -> Option<String> {
    ISSUER.read().ok()?.clone()
}

/// The issuer's vox endpoint, as a URL a lane can dial.
///
/// `https` → `wss` and `http` → `ws`, because the issuer advertises the
/// origin it is served from and a WebSocket needs the other scheme.
/// Getting this wrong is a dial that fails with a scheme error rather
/// than anything about auth, so it is worth doing in one place.
#[must_use]
pub fn issuer_vox() -> Option<String> {
    let base = issuer()?;
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // Already a ws/wss URL, or something we should not rewrite.
        base
    };
    Some(format!("{ws}/vox"))
}

#[cfg(test)]
mod tests {
    use super::{issuer, issuer_vox, note};

    /// Serialised: the registry is process-wide, and these tests set it.
    fn with_issuer<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        note(value.map(std::borrow::ToOwned::to_owned));
        let out = f();
        note(None);
        out
    }

    #[test]
    fn no_issuer_is_the_self_hosted_default() {
        with_issuer(None, || {
            assert_eq!(issuer(), None);
            assert_eq!(issuer_vox(), None, "nothing to dial");
        });
    }

    /// The scheme swap. A dial against `https://…` fails on the scheme,
    /// which reads as a connection problem rather than a URL one.
    #[test]
    fn an_https_issuer_is_dialled_over_wss() {
        with_issuer(Some("https://auth.fasttrackstudio.app"), || {
            assert_eq!(
                issuer_vox().as_deref(),
                Some("wss://auth.fasttrackstudio.app/vox")
            );
        });
    }

    #[test]
    fn a_local_issuer_is_dialled_over_ws() {
        with_issuer(Some("http://127.0.0.1:8099"), || {
            assert_eq!(issuer_vox().as_deref(), Some("ws://127.0.0.1:8099/vox"));
        });
    }

    /// A trailing slash would make the path `//vox`, which some gateways
    /// route differently and none route better.
    #[test]
    fn a_trailing_slash_does_not_double_up() {
        with_issuer(Some("https://auth.example.app/"), || {
            assert_eq!(issuer_vox().as_deref(), Some("wss://auth.example.app/vox"));
        });
    }

    /// Taking identity back must take effect, not leave the old issuer
    /// answering — sign-in would go somewhere this server no longer
    /// trusts.
    #[test]
    fn clearing_the_issuer_takes_effect() {
        with_issuer(Some("https://auth.example.app"), || {
            assert!(issuer().is_some());
            note(None);
            assert_eq!(issuer(), None);
        });
    }

    /// An empty string is not an issuer. A server that sends `""` (or a
    /// field that serialises to it) means "none", and treating it as a
    /// URL would send sign-in to `ws:///vox`.
    #[test]
    fn an_empty_issuer_is_no_issuer() {
        with_issuer(Some("   "), || assert_eq!(issuer(), None));
    }
}
