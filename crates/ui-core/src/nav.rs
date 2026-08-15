//! Navigation seam for feature UI crates.
//!
//! The router lives in the shell (`ui::routes::Route`), and it must —
//! a feature crate cannot name a route variant without depending on
//! every other feature's route parameters. But a feature page still
//! needs to link *out* of itself: the scripture reader links a backlink
//! to the vault note it came from.
//!
//! So the shell hands down href builders as contexts. The shell owns
//! the URL (it renders the typed route to a string), the feature crate
//! owns nothing but the anchor.
//!
//! ```ignore
//! // shell, once, next to the other chrome contexts:
//! use_context_provider(|| NoteHref(Callback::new(|path: String| {
//!     Route::VaultRoute { path, org: String::new() }.to_string()
//! })));
//!
//! // feature crate:
//! let href = use_note_href();
//! rsx! { Link { to: href(path.clone()), "{title}" } }
//! ```

use dioxus::prelude::*;

/// Builds the URL of a vault note from its path.
#[derive(Clone, Copy)]
pub struct NoteHref(pub Callback<String, String>);

/// The shell's vault-note href builder. Falls back to `/vault/<path>`
/// when no shell provided one (isolated component previews), so a
/// feature crate never has to handle a missing context.
#[must_use]
pub fn use_note_href() -> Callback<String, String> {
    try_use_context::<NoteHref>()
        .map(|h| h.0)
        .unwrap_or_else(|| use_callback(|path: String| format!("/vault/{path}")))
}
