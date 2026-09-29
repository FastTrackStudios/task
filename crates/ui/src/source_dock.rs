//! The source dock: a citation's video, playing where you are reading.
//!
//! A source badge with a timestamp (`[[talk#^t1226|20:26]]`) on a YouTube
//! source does not navigate: it opens this small player docked at the
//! bottom corner, started at that second. Reading on, another timestamp
//! re-cues the same dock; the ✕ closes it, and "Open source" goes to the
//! source's page (transcript, notes, chapters) for the whole thing.
//!
//! One dock for the whole app, provided by the shell; any page's link
//! handler opens it through [`use_source_dock`]. On a phone it sits full
//! width above the tab bar.

use dioxus::prelude::*;

use crate::routes::Route;

/// What the dock is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct DockedSource {
    /// YouTube video id.
    pub video: String,
    /// Where to start, in seconds.
    pub start: u64,
    /// The source's short name, for the dock's head.
    pub title: String,
    /// The source's own page.
    pub open: Route,
}

#[derive(Clone, Copy)]
struct SourceDock(Signal<Option<DockedSource>>);

/// Provide the dock (once, in the shell).
pub fn provide_source_dock() {
    use_context_provider(|| SourceDock(Signal::new(None)));
}

/// The dock's state: `set(Some(..))` plays, `set(None)` closes. `None`
/// outside a shell (a shared page), where citations navigate as links.
#[must_use]
pub fn use_source_dock() -> Option<Signal<Option<DockedSource>>> {
    try_consume_context::<SourceDock>().map(|d| d.0)
}

/// The dock itself (rendered once, by the shell).
#[component]
pub fn SourceDockView() -> Element {
    let Some(mut dock) = use_source_dock() else {
        return rsx! {};
    };
    let Some(src) = dock.read().clone() else {
        return rsx! {};
    };
    let at = clock(src.start);
    // Keyed by video and start, so a new timestamp re-cues the player.
    let key = format!("{}@{}", src.video, src.start);
    rsx! {
        div {
            class: "fixed inset-x-2 z-40 overflow-hidden rounded-xl border border-border bg-card shadow-2xl md:inset-x-auto md:right-4 md:bottom-4 md:w-[24rem]",
            style: "bottom: calc(4rem + env(safe-area-inset-bottom, 0px));",
            "data-testid": "source-dock",
            div { class: "flex items-center gap-2 border-b border-border px-3 py-1.5 text-xs",
                span { class: "min-w-0 flex-1 truncate font-medium text-foreground", "{src.title}" }
                span { class: "shrink-0 font-mono tabular-nums text-muted-foreground", "{at}" }
                Link {
                    to: src.open.clone(),
                    class: "shrink-0 rounded px-1.5 py-0.5 text-muted-foreground hover:bg-accent hover:text-foreground",
                    onclick: move |_| dock.set(None),
                    "Open source"
                }
                button {
                    r#type: "button",
                    class: "flex h-7 w-7 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground",
                    "aria-label": "Close player",
                    onclick: move |_| dock.set(None),
                    "✕"
                }
            }
            iframe {
                key: "{key}",
                class: "aspect-video w-full bg-black",
                src: "https://www.youtube.com/embed/{src.video}?start={src.start}&autoplay=1&rel=0",
                allow: "autoplay; encrypted-media; picture-in-picture",
                allowfullscreen: true,
            }
        }
    }
}

/// `1226` → `20:26`, `3725` → `1:02:05`.
fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The seconds in a `#^t1226` citation anchor.
#[must_use]
pub fn anchor_seconds(href: &str) -> Option<u64> {
    let rest = href.split_once("#^t")?.1;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_citation_anchor_gives_its_second() {
        assert_eq!(super::anchor_seconds("talk-ac25#^t1226"), Some(1226));
        assert_eq!(super::anchor_seconds("talk-ac25#^t46-note2"), Some(46));
        assert_eq!(super::anchor_seconds("talk-ac25#Heading"), None);
        assert_eq!(super::clock(3725), "1:02:05");
    }
}
