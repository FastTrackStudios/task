//! Document tabs and split panes for the vault page.
//!
//! A pane is an ordered set of open tabs plus the active index; the
//! page holds 1..=[`MAX_PANES`] of them. Only the active tab of each
//! pane is mounted, so switching tabs remounts a fresh `NoteView` (and
//! therefore a fresh `DocumentSession`).

use dioxus::prelude::*;

use super::tree::basename_of;

/// Hard cap on side-by-side panes — a single 2-way horizontal split.
pub(super) const MAX_PANES: usize = 2;

/// One pane's tab strip: a button per open tab (active = primary
/// underline; a dimmer underline marks the active tab of an
/// *unfocused* pane), each with a close ✕, plus split / close-pane
/// controls docked at the right edge.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_tab_bar(
    pi: usize,
    pane: &Pane,
    n_panes: usize,
    focused: usize,
    focus_tab: Callback<(usize, usize)>,
    close_tab: Callback<(usize, usize)>,
    split: Callback<()>,
    close_pane: Callback<usize>,
) -> Element {
    let is_focused_pane = focused == pi;
    rsx! {
        div { class: "flex shrink-0 items-center gap-0.5 overflow-x-auto border-b border-border/60 bg-muted/20 px-1",
            for (idx, tab) in pane.tabs.iter().cloned().enumerate() {
                {
                    let is_active = idx == pane.active;
                    let title = basename_of(&tab.path).to_owned();
                    let cls = if is_active && is_focused_pane {
                        "flex items-center gap-1 border-b-2 border-primary px-2 py-1.5 text-xs font-medium text-foreground"
                    } else if is_active {
                        "flex items-center gap-1 border-b-2 border-border px-2 py-1.5 text-xs font-medium text-foreground"
                    } else {
                        "flex items-center gap-1 border-b-2 border-transparent px-2 py-1.5 text-xs text-muted-foreground hover:text-foreground"
                    };
                    rsx! {
                        div { key: "{tab.path}", class: cls,
                            button {
                                class: "min-w-0 max-w-[12rem] truncate text-left",
                                title: "{tab.path}",
                                onclick: move |_| focus_tab.call((pi, idx)),
                                "{title}"
                            }
                            button {
                                class: "shrink-0 rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground",
                                title: "Close tab",
                                onclick: move |_| close_tab.call((pi, idx)),
                                "×"
                            }
                        }
                    }
                }
            }
            div { class: "ml-auto flex shrink-0 items-center gap-0.5 pl-1",
                if n_panes < MAX_PANES {
                    button {
                        class: "rounded px-1.5 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: "Split right",
                        onclick: move |_| split.call(()),
                        "⇥"
                    }
                }
                if n_panes > 1 {
                    button {
                        class: "rounded px-1.5 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: "Close pane",
                        onclick: move |_| close_pane.call(pi),
                        "⊟"
                    }
                }
            }
        }
    }
}

/// One open note in a pane's tab strip: its path + last-known sha
/// (the `DocumentSession` conditional-write base).
#[derive(Clone, PartialEq)]
pub(super) struct OpenTab {
    pub(super) path: String,
    pub(super) sha: String,
}

/// A document pane — an ordered set of open tabs and the active one.
/// Each pane mounts exactly its active tab's [`NoteView`] (inactive
/// tabs are unmounted; switching remounts a fresh session).
#[derive(Clone, PartialEq)]
pub(super) struct Pane {
    pub(super) tabs: Vec<OpenTab>,
    pub(super) active: usize,
}
