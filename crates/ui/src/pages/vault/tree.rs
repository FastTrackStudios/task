//! The vault page's virtual-folder tree: the `folder:`-frontmatter
//! derived node model and its recursive renderer.
//!
//! "Virtual" because the tree is not the on-disk layout — notes live
//! flat and declare a `folder` property, so the same note can be
//! re-filed without moving the file. `build_tree` folds the folder
//! index into nodes; `render_node` draws one node and recurses.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use architect_ui::lucide_dioxus::{ChevronRight, FileText, Folder};
use dioxus::prelude::*;
use vault_proto::PageMeta;

use super::FileMeta;

/// Render one tree node (and, when a folder is expanded, its
/// children) recursively. Folders: chevron toggles `collapsed`,
/// the name opens the folder note, "+" files a new note under
/// it. Leaves: the name opens the note. Every row has a hover
/// "move" affordance.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_node(
    nodes: Rc<Vec<TreeNode>>,
    idx: usize,
    depth: usize,
    mut collapsed: Signal<HashSet<String>>,
    selected: Memo<Option<String>>,
    on_open: Callback<FileMeta>,
    mut move_target: Signal<Option<String>>,
    mut create_parent: Signal<Option<String>>,
) -> Element {
    let node = nodes[idx].clone();
    let key = node.meta.basename.to_lowercase();
    let is_expanded = !collapsed.read().contains(&key);
    let is_active = selected.read().as_deref() == Some(node.meta.path.as_str());
    let indent = depth * 14 + 8;

    let open_meta = FileMeta {
        path: node.meta.path.clone(),
        sha256: node.meta.sha256.clone(),
    };
    let move_path = node.meta.path.clone();
    let create_base = node.meta.basename.clone();
    let toggle_key = key.clone();

    let row_cls = if is_active {
        "group flex items-center gap-1 rounded pr-1 text-sm bg-accent text-accent-foreground"
    } else {
        "group flex items-center gap-1 rounded pr-1 text-sm hover:bg-accent/50"
    };

    rsx! {
        div { key: "{node.meta.path}",
            div { class: row_cls, style: "padding-left: {indent}px",
                if node.is_folder {
                    button {
                        class: "flex size-5 shrink-0 items-center justify-center text-muted-foreground",
                        onclick: move |_| {
                            let mut c = collapsed.write();
                            if !c.remove(&toggle_key) { c.insert(toggle_key.clone()); }
                        },
                        span {
                            class: if is_expanded { "transition-transform rotate-90" } else { "transition-transform" },
                            ChevronRight { size: 14 }
                        }
                    }
                    span { class: "flex size-4 shrink-0 items-center justify-center text-muted-foreground",
                        Folder { size: 14 }
                    }
                } else {
                    span { class: "ml-5 flex size-4 shrink-0 items-center justify-center text-muted-foreground",
                        FileText { size: 14 }
                    }
                }
                button {
                    class: "min-w-0 flex-1 truncate py-1 text-left",
                    onclick: move |_| on_open.call(open_meta.clone()),
                    "{node.meta.title}"
                }
                if node.is_folder {
                    button {
                        class: "hidden size-5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground group-hover:flex",
                        title: "New note in this folder",
                        onclick: move |_| create_parent.set(Some(create_base.clone())),
                        "+"
                    }
                }
                button {
                    class: "hidden size-5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground group-hover:flex",
                    title: "Move to folder",
                    onclick: move |_| move_target.set(Some(move_path.clone())),
                    "⋯"
                }
            }
            if node.is_folder && is_expanded {
                for &child in node.children.iter() {
                    {render_node(nodes.clone(), child, depth + 1, collapsed, selected, on_open, move_target, create_parent)}
                }
            }
        }
    }
}

/// Build the virtual-folder tree from the flat page list.
/// Parent = each page's `folder` (already a basename); roots are
/// pages with no/unresolved parent. Cycles are broken (the node
/// falls back to a root). Children sort folders-first, then by
/// title.
pub(crate) fn build_tree(pages: &[PageMeta]) -> (Vec<TreeNode>, Vec<usize>) {
    let mut nodes: Vec<TreeNode> = pages
        .iter()
        .map(|m| TreeNode {
            meta: m.clone(),
            children: Vec::new(),
            is_folder: false,
        })
        .collect();

    // basename (lowercased) → first node with it.
    let mut by_base: HashMap<String, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        by_base.entry(n.meta.basename.to_lowercase()).or_insert(i);
    }

    // Resolve each node's parent index (None = root). Self-parent
    // and unknown targets resolve to root.
    let parent_of: Vec<Option<usize>> = (0..nodes.len())
        .map(|i| {
            let f = nodes[i].meta.folder.to_lowercase();
            if f.is_empty() {
                return None;
            }
            match by_base.get(&f) {
                Some(&p) if p != i => Some(p),
                _ => None,
            }
        })
        .collect();

    // A node is a tree child only if walking its ancestry reaches
    // a root within N steps — otherwise it's in a cycle and we
    // treat it as a root so the tree stays finite.
    let max = nodes.len();
    let resolves = |start: usize| -> bool {
        let mut cur = start;
        for _ in 0..=max {
            match parent_of[cur] {
                None => return true,
                Some(p) => cur = p,
            }
        }
        false
    };

    let mut roots = Vec::new();
    for (i, parent) in parent_of.iter().enumerate() {
        match parent {
            Some(p) if resolves(i) => nodes[*p].children.push(i),
            _ => roots.push(i),
        }
    }

    for n in &mut nodes {
        let t = n.meta.page_type.to_lowercase();
        n.is_folder = !n.children.is_empty() || t == "folder" || t == "index";
    }

    // Sort key per node — captured up front so the child/root
    // sorts borrow it (not `nodes`).
    let sort_key: Vec<(bool, String)> = nodes
        .iter()
        .map(|n| (!n.is_folder, n.meta.title.to_lowercase()))
        .collect();
    roots.sort_by(|a, b| sort_key[*a].cmp(&sort_key[*b]));
    for n in &mut nodes {
        n.children.sort_by(|a, b| sort_key[*a].cmp(&sort_key[*b]));
    }

    (nodes, roots)
}

/// Filename without dirs/extension — display fallback for paths
/// missing from the folder index. Also the note title shown +
/// edited by the [`note_header`](crate::pages::note_header) H1.
pub(crate) fn basename_of(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".md").unwrap_or(file)
}

/// One node of the virtual-folder tree.
#[derive(Clone, PartialEq)]
pub(crate) struct TreeNode {
    pub(crate) meta: PageMeta,
    pub(crate) children: Vec<usize>,
    pub(crate) is_folder: bool,
}
