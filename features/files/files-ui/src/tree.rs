//! [`TreeExplorer`] — the org tree (issue #304) in the Files pane:
//! the same unified namespace the WebDAV mount serves, browsed with
//! the explorer's own tiles. When a path resolves to a File Root
//! (`Projects/<name>/Media/…`), the full root [`crate::Explorer`]
//! mounts in place — inspector, versions, review and all.

use dioxus::prelude::*;
use files_proto::{BrowseEntry, FilesEvent, TreeNode};
use uuid::Uuid;

use crate::Location;
use crate::explorer::GridTile;

/// The four areas the sidebar offers. `as_str` doubles as the tree
/// path root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeArea {
    Projects,
    Vault,
    Wiki,
    Assets,
}

impl TreeArea {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::Vault => "Vault",
            Self::Wiki => "Wiki",
            Self::Assets => "Assets",
        }
    }

    pub const ALL: [Self; 4] = [Self::Projects, Self::Vault, Self::Wiki, Self::Assets];
}

async fn tree_browse(org: &str, path: &str) -> Result<TreeNode, String> {
    crate::client(org)
        .await?
        .tree_browse(path.to_owned())
        .await
        .map_err(|e| e.to_string())
}

/// The tree browser for one area. Keyed by (org, area) at the mount,
/// so switches remount with fresh state.
#[component]
pub fn TreeExplorer(org: String, area: TreeArea) -> Element {
    // The path INSIDE the tree, always rooted at the area.
    let mut path = use_signal(|| area.as_str().to_string());
    let mut selected = use_signal(|| Option::<String>::None);

    let node = {
        let org = org.clone();
        use_resource(move || {
            let org = org.clone();
            let path = path.read().clone();
            async move { tree_browse(&org, &path).await }
        })
    };

    // Anything that can move a listing anywhere in the org re-reads —
    // the tree spans every root plus the vault, so per-root filtering
    // would under-refresh the join and the lenses.
    {
        let org = org.clone();
        crate::use_files_events(
            move || org.clone(),
            move |event: FilesEvent| {
                let mut node = node;
                match event {
                    FilesEvent::ReviewCreated(_)
                    | FilesEvent::ReviewCommentAdded(_)
                    | FilesEvent::ReviewCommentDeleted(_) => {}
                    _ => node.restart(),
                }
            },
        );
    }

    let crumbs: Vec<(String, String)> = {
        let full = path.read().clone();
        let mut acc = String::new();
        full.split('/')
            .filter(|s| !s.is_empty())
            .map(|seg| {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                (seg.to_string(), acc.clone())
            })
            .collect()
    };

    rsx! {
        div { class: "flex h-full min-h-0 flex-col gap-2 p-3",
            // ── crumbs ──────────────────────────────────────────
            div { class: "flex items-center gap-1 text-xs text-muted-foreground flex-wrap",
                for (i , (label , to)) in crumbs.iter().cloned().enumerate() {
                    if i > 0 {
                        span { "/" }
                    }
                    button {
                        class: "rounded px-1.5 py-0.5 hover:bg-muted/50",
                        onclick: move |_| {
                            path.set(to.clone());
                            selected.set(None);
                        },
                        "{label}"
                    }
                }
            }
            // ── body ────────────────────────────────────────────
            div { class: "min-h-0 flex-1 overflow-y-auto",
                {match &*node.read_unchecked() {
                    None => rsx! {
                        task_ui_core::states::LoadingState { rows: 3 }
                    },
                    Some(Err(e)) => rsx! {
                        task_ui_core::states::ErrorState {
                            message: e.clone(),
                            on_retry: {
                                let mut node = node;
                                move |()| node.restart()
                            },
                        }
                    },
                    // A root handoff: the subtree IS a File Root — the
                    // full explorer takes over (inspector, versions,
                    // review), pinned so it can't wander above the
                    // tree's own entry point.
                    Some(Ok(TreeNode::Root { id, subpath })) => rsx! {
                        RootHandoff {
                            org: org.clone(),
                            root_id: *id,
                            subpath: subpath.clone(),
                        }
                    },
                    Some(Ok(TreeNode::Listing(entries))) if entries.is_empty() => rsx! {
                        task_ui_core::states::EmptyState {
                            title: "Nothing here",
                            hint: "This part of the tree is empty so far.",
                        }
                    },
                    Some(Ok(TreeNode::Listing(entries))) => rsx! {
                        div { class: "grid gap-1", style: "grid-template-columns:repeat(auto-fill,minmax(108px,1fr))",
                            for entry in entries.iter().cloned() {
                                GridTile {
                                    key: "{entry.name}",
                                    entry: entry.clone(),
                                    selected: selected.read().as_deref() == Some(entry.name.as_str()),
                                    on_select: {
                                        let name = entry.name.clone();
                                        move |()| selected.set(Some(name.clone()))
                                    },
                                    on_open: {
                                        let entry: BrowseEntry = entry.clone();
                                        move |()| {
                                            if entry.is_dir {
                                                let next = format!("{}/{}", path.peek(), entry.name);
                                                path.set(next);
                                                selected.set(None);
                                            }
                                        }
                                    },
                                }
                            }
                        }
                    },
                }}
            }
        }
    }
}

/// The mounted root explorer for a `…/Media/…` handoff. Fetches the
/// root's info so the header (name, flavor, share) renders like the
/// direct-root view.
#[component]
fn RootHandoff(org: String, root_id: Uuid, subpath: String) -> Element {
    let info = {
        let org = org.clone();
        use_resource(move || {
            let org = org.clone();
            async move {
                crate::client(&org)
                    .await?
                    .get_root(root_id)
                    .await
                    .map_err(|e| e.to_string())
            }
        })
    };
    rsx! {
        {match &*info.read_unchecked() {
            None => rsx! {
                task_ui_core::states::LoadingState { rows: 2 }
            },
            Some(Err(e)) => rsx! {
                div { class: "text-xs text-destructive", "Couldn't open the project's media: {e}" }
            },
            Some(Ok(root)) => rsx! {
                crate::Explorer {
                    key: "{org}:{root_id}:{subpath}",
                    org: org.clone(),
                    start: Location::Root {
                        id: root_id,
                        subpath: subpath.clone(),
                    },
                    floor: subpath.clone(),
                    root: root.clone(),
                }
            },
        }}
    }
}
