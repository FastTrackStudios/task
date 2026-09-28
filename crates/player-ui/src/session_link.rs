//! The door from a song or setlist note into Session.
//!
//! Rehearsing a song — its stems, mixer, engraved chart, the section
//! timeline, the fullscreen performance view — is Session's job, and
//! Session is its own app (`session.fasttrackstudio.app`). Task keeps
//! what a song IS (the note, its manifest, its media) and the one thing a
//! listener needs without leaving: the global Now Playing stream. Anything
//! more is a link across.

/// Where the Session app is served.
pub const SESSION_APP: &str = "https://session.fasttrackstudio.app/app/";

/// What a link opens in Session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTarget<'a> {
    Song(&'a str),
    Setlist(&'a str),
}

/// The Session URL for a song or setlist of `org`, by slug.
#[must_use]
pub fn session_url(org: &str, target: SessionTarget<'_>) -> String {
    let (kind, slug) = match target {
        SessionTarget::Song(slug) => ("song", slug),
        SessionTarget::Setlist(slug) => ("setlist", slug),
    };
    format!("{SESSION_APP}?org={}&{kind}={}", encode(org), encode(slug))
}

/// Open `url` in a new browser tab. Off the web there is no tab to open,
/// so the click is logged and dropped.
pub fn open_in_new_tab(url: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let opened = web_sys::window()
            .map(|w| w.open_with_url_and_target(url, "_blank").is_ok())
            .unwrap_or(false);
        if !opened {
            tracing::warn!(url, "session link: the browser refused to open a tab");
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    tracing::info!(url, "session link: no browser tab to open on this target");
}

/// Percent-encode a query value (slugs and org slugs are already
/// URL-safe; this keeps a hand-written title from breaking the query).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_link_names_the_org_and_the_song() {
        assert_eq!(
            session_url("acme-audio", SessionTarget::Song("first-light")),
            "https://session.fasttrackstudio.app/app/?org=acme-audio&song=first-light"
        );
    }

    #[test]
    fn a_setlist_title_is_encoded() {
        assert_eq!(
            session_url("rockstars", SessionTarget::Setlist("Fall Show #2")),
            "https://session.fasttrackstudio.app/app/?org=rockstars&setlist=Fall%20Show%20%232"
        );
    }
}
