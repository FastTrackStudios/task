//! The vault page's virtual-folder tree: the `folder:`-frontmatter
//! derived node model and its recursive renderer.
//!
//! "Virtual" because the tree is not the on-disk layout — notes live
//! flat and declare a `folder` property, so the same note can be
//! re-filed without moving the file. `build_tree` folds the folder
//! index into nodes; [`group_by_directory`] gathers what is left at the
//! root by the directory it lives in (the app's own stores — tasks,
//! inbox records — so they don't bury the notes); `render_node` draws
//! one node and recurses.
//!
//! Rows are sized for a thumb below `md:` (44pt, 16px, icons that read)
//! and for a pointer above it (Obsidian's compact explorer).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use architect_ui::lucide_dioxus::{ChevronRight, FileText, Folder, FolderOpen};
use dioxus::prelude::*;
use vault_proto::PageMeta;

use super::FileMeta;

/// The `collapsed` key that marks a directory group as *opened* — groups
/// start closed (a store of 18 tasks should not push every note down),
/// folders start open.
pub(super) fn group_open_key(dir: &str) -> String {
    format!("+{}", dir.to_lowercase())
}

/// Render one tree node (and, when a folder is expanded, its
/// children) recursively. Folders: the chevron toggles `collapsed`,
/// the name opens the folder note, "+" files a new note under
/// it. Directory groups: the whole row toggles. Leaves: the row opens
/// the note. Every note row has a hover "move" affordance (desk).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_node(
    nodes: Rc<Vec<TreeNode>>,
    idx: usize,
    depth: usize,
    collapsed: Signal<HashSet<String>>,
    selected: Memo<Option<String>>,
    on_open: Callback<FileMeta>,
    mut move_target: Signal<Option<String>>,
    mut create_parent: Signal<Option<String>>,
) -> Element {
    let node = nodes[idx].clone();
    let key = if node.group {
        group_open_key(&node.meta.title)
    } else {
        node.meta.basename.to_lowercase()
    };
    let is_expanded = collapsed.read().contains(&key) == node.group;
    let is_active = !node.group && selected.read().as_deref() == Some(node.meta.path.as_str());
    // Phone indent is wider: a thumb needs to see the nesting.
    let indent = depth * 16;

    let open_meta = FileMeta {
        path: node.meta.path.clone(),
        sha256: node.meta.sha256.clone(),
    };
    let move_path = node.meta.path.clone();
    let create_base = node.meta.basename.clone();
    let count = node.children.len();

    let row_cls = if is_active {
        "group flex min-h-11 items-center gap-3 rounded-xl pl-3 pr-2 text-[16px] bg-foreground/[0.08] text-foreground md:min-h-0 md:gap-1 md:rounded md:pl-2 md:pr-1 md:text-sm md:bg-accent md:text-accent-foreground"
    } else {
        "group flex min-h-11 items-center gap-3 rounded-xl pl-3 pr-2 text-[16px] text-foreground active:bg-foreground/[0.06] md:min-h-0 md:gap-1 md:rounded md:pl-2 md:pr-1 md:text-sm md:hover:bg-accent/50"
    };
    let icon_cls = "flex size-5 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-[18px] md:size-4 md:[&_svg]:size-3.5";

    rsx! {
        div { key: "{node.meta.path}",
            div { class: row_cls, style: "margin-left: {indent}px",
                if node.group {
                    // A directory group: one target, the whole row.
                    button {
                        r#type: "button",
                        class: "flex min-h-11 min-w-0 flex-1 items-center gap-3 text-left md:min-h-0 md:gap-1 md:py-1",
                        "aria-expanded": "{is_expanded}",
                        onclick: toggler(collapsed, key.clone()),
                        span { class: icon_cls,
                            if is_expanded { FolderOpen { size: 18 } } else { Folder { size: 18 } }
                        }
                        span { class: "min-w-0 flex-1 truncate font-medium", "{node.meta.title}" }
                        span { class: "shrink-0 text-[13px] tabular-nums text-muted-foreground md:text-xs", "{count}" }
                        span {
                            class: if is_expanded { "shrink-0 text-muted-foreground transition-transform rotate-90" } else { "shrink-0 text-muted-foreground transition-transform" },
                            ChevronRight { size: 16 }
                        }
                    }
                } else {
                    span { class: icon_cls,
                        if node.is_folder { Folder { size: 18 } } else { FileText { size: 18 } }
                    }
                    button {
                        r#type: "button",
                        class: "min-h-11 min-w-0 flex-1 truncate text-left md:min-h-0 md:py-1",
                        onclick: move |_| on_open.call(open_meta.clone()),
                        "{node.meta.title}"
                    }
                    if node.is_folder {
                        button {
                            r#type: "button",
                            class: "hidden size-5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground md:group-hover:flex",
                            title: "New note in this folder",
                            onclick: move |_| create_parent.set(Some(create_base.clone())),
                            "+"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "hidden size-5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground md:group-hover:flex",
                        title: "Move to folder",
                        onclick: move |_| move_target.set(Some(move_path.clone())),
                        "⋯"
                    }
                    if node.is_folder {
                        button {
                            r#type: "button",
                            class: "flex size-9 shrink-0 items-center justify-center rounded-lg text-muted-foreground active:bg-foreground/10 md:size-5",
                            "aria-label": if is_expanded { "Collapse" } else { "Expand" },
                            onclick: toggler(collapsed, key.clone()),
                            span {
                                class: if is_expanded { "transition-transform rotate-90" } else { "transition-transform" },
                                ChevronRight { size: 16 }
                            }
                        }
                    }
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

/// A click that opens or closes the node keyed `key`.
fn toggler(mut collapsed: Signal<HashSet<String>>, key: String) -> impl FnMut(MouseEvent) {
    move |_| {
        let mut c = collapsed.write();
        if !c.remove(&key) {
            c.insert(key.clone());
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
            group: false,
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

/// Gather the root pages that live in a directory under one group per
/// top-level directory (`Task/…`, `Records/…`), after the notes at the
/// vault's root and its virtual folders. Groups are display-only nodes
/// (`group: true`, no file behind them) and sort by name.
pub(crate) fn group_by_directory(
    (mut nodes, roots): (Vec<TreeNode>, Vec<usize>),
) -> (Vec<TreeNode>, Vec<usize>) {
    let mut kept = Vec::new();
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for r in roots {
        let n = &nodes[r];
        let dir = n.meta.path.split_once('/').map(|(d, _)| d.to_owned());
        match dir {
            Some(dir) if !n.is_folder => match groups.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, members)) => members.push(r),
                None => groups.push((dir, vec![r])),
            },
            _ => kept.push(r),
        }
    }
    groups.sort_by_key(|(d, _)| d.to_lowercase());
    for (dir, members) in groups {
        nodes.push(TreeNode {
            meta: PageMeta {
                path: format!("{dir}/"),
                basename: dir.clone(),
                title: dir,
                page_type: "group".to_owned(),
                folder: String::new(),
                tags: Vec::new(),
                icon: String::new(),
                sha256: String::new(),
                aliases: Vec::new(),
            },
            children: members,
            is_folder: true,
            group: true,
        });
        kept.push(nodes.len() - 1);
    }
    (nodes, kept)
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
    /// A directory group ([`group_by_directory`]): no note behind it.
    pub(crate) group: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str, folder: &str) -> PageMeta {
        PageMeta {
            path: path.to_owned(),
            basename: basename_of(path).to_owned(),
            title: basename_of(path).to_owned(),
            page_type: String::new(),
            folder: folder.to_owned(),
            tags: Vec::new(),
            icon: String::new(),
            sha256: String::new(),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn stores_gather_under_their_directory_after_the_notes() {
        let pages = vec![
            page("Task/b.md", ""),
            page("Untitled.md", ""),
            page("Records/inbox/x.md", ""),
            page("Task/a.md", ""),
            page("Sub.md", "Untitled"),
        ];
        let (nodes, roots) = group_by_directory(build_tree(&pages));
        let titles: Vec<_> = roots
            .iter()
            .map(|&r| nodes[r].meta.title.as_str())
            .collect();
        // The root note (a folder now, holding Sub) first, then the groups.
        assert_eq!(titles, ["Untitled", "Records", "Task"]);
        let task = &nodes[roots[2]];
        assert!(task.group && task.is_folder);
        let members: Vec<_> = task
            .children
            .iter()
            .map(|&c| nodes[c].meta.title.as_str())
            .collect();
        assert_eq!(members, ["a", "b"]);
    }
}
