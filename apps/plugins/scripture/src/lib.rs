//! Scripture, as a Task app.
//!
//! The reader itself is unchanged — it still lives in
//! `features/scripture/scripture-ui`, because a screen that big is a
//! feature crate whoever mounts it. What moved here is everything that
//! used to make the *shell* know about scripture: a route in Task's
//! `Route` enum, a nav entry in Task's nav, and a branch in Task's link
//! handler that recognised `John 3:16` by hand.
//!
//! That is the whole migration, and it is worth naming what it buys.
//! Before, adding scripture meant editing four files in `crates/ui`;
//! an org that did not want it still compiled it. Now the app binary
//! registers it in one line and nothing else in Task has heard of it:
//!
//! ```ignore
//! task_plugin_ui::register(task_plugin_scripture::APP);
//! ```
//!
//! ## What is still not here
//!
//! Honestly: three things. `crates/ui` still knows about scripture in
//! [`vault_lookup`] (the `scripture://` content scheme that renders a
//! passage inline in a note), in `feeds.rs` (a reading-plan feed), and
//! in the vault graph (turning `John.3.16` into `John 3:16` on a node
//! label). Each needs a contribution kind that does not exist yet, and
//! inventing three of them for one app is how you get a plugin seam
//! shaped like exactly one plugin. They come when a second app wants
//! the same thing — the player wants the graph one already.
//!
//! [`vault_lookup`]: https://example.invalid

use scripture_proto::reference::ScriptureRef;
use task_plugin_ui::architect_ui::lucide_dioxus::BookOpen;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{Claim, LinkTarget, PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "scripture",
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Scripture",
        icon: icon,
        path: "",
    }],
    view: view,
    widgets: None,
    fences: None,
    claim_link: Some(claim_link),
    claim_href: Some(claim_href),
};

/// A wikilink that parses as a reference **is** the passage.
///
/// This is the claim the whole priority mechanism was built for. A note
/// called `John 3:16` is somebody writing *about* the verse; it is not
/// a thing that should shadow the verse when another note links to it.
/// So it is [`Claim::Always`] and beats a page.
///
/// The safety comes from the parser, not from a list kept here.
/// [`ScriptureRef::parse`] requires a book **and** a chapter **and** a
/// verse — `John` alone is a parse error. That matters more than it
/// looks: John, Mark, Philemon and James are all perfectly ordinary
/// names for a note about a person, and a claim on bare book names
/// would quietly eat every one of them. Requiring the numbers is what
/// makes "always wins" safe to say.
fn claim_link(text: &str) -> Option<Claim> {
    ScriptureRef::parse(text)
        .ok()
        .map(|_| Claim::Always(passage(text)))
}

/// This app's own scheme, for widgets and generated notes.
fn claim_href(href: &str) -> Option<LinkTarget> {
    let reference = href.strip_prefix("scripture-open:")?.trim();
    ScriptureRef::parse(reference).ok().map(|_| passage(reference))
}

/// The reader, positioned at a reference.
///
/// Encoded through the SDK rather than by hand — references carry
/// spaces (`John 3:16-20`) and an `@` for a translation qualifier.
fn passage(reference: &str) -> LinkTarget {
    LinkTarget::param("reference", reference)
}

fn icon() -> Element {
    rsx! { BookOpen { size: 16 } }
}

fn view(path: &str, query: &str) -> Option<Element> {
    match path {
        // An empty reference is not an error — the reader opens where it
        // opens (John 1) when nobody asked for anything in particular.
        "" => {
            let reference = task_plugin_ui::query_param(query, "reference").unwrap_or_default();
            Some(rsx! { scripture_ui::ScriptureView { reference } })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_always_beats_a_note_of_the_same_name() {
        let claim = claim_link("John 3:16").expect("a reference is claimed");
        assert!(claim.beats_a_page());
    }

    /// The bug this app would otherwise introduce for everybody with a
    /// friend named John.
    #[test]
    fn a_bare_book_name_is_left_to_the_vault() {
        for name in ["John", "Mark", "James", "Philemon"] {
            assert!(
                claim_link(name).is_none(),
                "{name} is a person's note as often as a book"
            );
        }
    }

    #[test]
    fn ordinary_notes_are_not_touched() {
        for text in ["Weekly Review", "Bolognese", "3:16", ""] {
            assert!(claim_link(text).is_none(), "{text:?} was claimed");
        }
    }

    #[test]
    fn a_range_and_a_translation_reach_the_reader_intact() {
        let claim = claim_link("John 3:16-20@ESV").expect("claimed");
        let query = &claim.target().query;
        assert_eq!(
            task_plugin_ui::query_param(query, "reference").as_deref(),
            Some("John 3:16-20@ESV")
        );
    }

    #[test]
    fn the_scheme_opens_the_reader() {
        let target = claim_href("scripture-open:Romans 8:28").expect("claimed");
        assert_eq!(
            task_plugin_ui::query_param(&target.query, "reference").as_deref(),
            Some("Romans 8:28")
        );
        assert!(claim_href("recipe-open:Bolognese").is_none());
        assert!(
            claim_href("scripture-open:not a reference").is_none(),
            "the scheme still has to name a passage"
        );
    }
}
