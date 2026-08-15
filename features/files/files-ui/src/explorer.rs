//! The explorer shell — the Spacedrive-style browsing surface.
//!
//! Layout: a toolbar (breadcrumbs, view toggle, inspector toggle),
//! the listing as a **grid** of tiles or a **list** of rows, and an
//! **inspector** sidebar for the selected entry — preview, metadata,
//! divergence resolution, and the file's version history.
//!
//! Interaction contract (the reference tool's):
//! - single click **selects** (the inspector shows the entry);
//! - double click **opens** (a folder navigates, a video file opens
//!   the quick preview);
//! - `Space` quick-previews the selection, `Enter` opens it,
//!   `Escape` closes the preview / clears the selection.
//!
//! The embedded (note-widget) mount keeps the same shell but stacks
//! the inspector *below* the listing — a note column has no width for
//! a sidebar.

use dioxus::prelude::*;
use files_proto::{
    BrowseEntry, ChainEntry, DivergenceChoice, DivergenceInfo, FileRootInfo, FilesEvent,
};
use architect_ui::lucide_dioxus::{
    Box as BoxIcon, File, FileText, Film, Folder, Image as ImageIcon, LayoutGrid, List as ListIcon,
    Music, PanelRight, X,
};
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::{
    Location, copy_to_clipboard, crumbs, fetch_chain, fetch_divergences, fetch_entries, human_size,
    mint_named_version_link, mint_share_link, name_a_version, project_version_label,
    resolve_a_divergence, restore_file, short_id,
};

/// How the listing renders. Persisted per mount only — a per-user
/// preference is later polish.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Grid,
    List,
}

/// What kind of thing an entry is, for icon + tint. Derived from the
/// extension — cheap, wrong is harmless (it only picks an icon).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Folder,
    Video,
    Audio,
    Image,
    Document,
    Project,
    Other,
}

pub(crate) fn entry_kind(entry: &BrowseEntry) -> EntryKind {
    if entry.is_dir {
        return EntryKind::Folder;
    }
    // The video list is the review player's own gate — ONE authority,
    // so an icon'd video always mounts a player and vice versa.
    if crate::review::is_video_path(&entry.name) {
        return EntryKind::Video;
    }
    // A real extension only: a file literally named "mp3" (no dot, a
    // common build-output name) must not classify by its whole name.
    let ext = match entry.name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => return EntryKind::Other,
    };
    match ext.as_str() {
        "wav" | "aif" | "aiff" | "flac" | "mp3" | "ogg" | "m4a" | "opus" => EntryKind::Audio,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tif" | "tiff" | "heic" | "svg" | "exr"
        | "dng" | "cr3" | "arw" => EntryKind::Image,
        "md" | "txt" | "pdf" | "rtf" | "doc" | "docx" => EntryKind::Document,
        "rpp" | "als" | "flp" | "logicx" | "ptx" | "cpr" | "drp" | "prproj" | "aep" | "blend"
        | "kdenlive" => EntryKind::Project,
        _ => EntryKind::Other,
    }
}

/// The kind's icon at `size`, in the kind's tint.
pub(crate) fn kind_icon(kind: EntryKind, size: usize) -> Element {
    match kind {
        EntryKind::Folder => rsx! {
            span { class: "text-sky-400", Folder { size } }
        },
        EntryKind::Video => rsx! {
            span { class: "text-purple-400", Film { size } }
        },
        EntryKind::Audio => rsx! {
            span { class: "text-emerald-400", Music { size } }
        },
        EntryKind::Image => rsx! {
            span { class: "text-amber-400", ImageIcon { size } }
        },
        EntryKind::Document => rsx! {
            span { class: "text-muted-foreground", FileText { size } }
        },
        EntryKind::Project => rsx! {
            span { class: "text-indigo-400", BoxIcon { size } }
        },
        EntryKind::Other => rsx! {
            span { class: "text-muted-foreground", File { size } }
        },
    }
}

fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Folder => "Folder",
        EntryKind::Video => "Video",
        EntryKind::Audio => "Audio",
        EntryKind::Image => "Image",
        EntryKind::Document => "Document",
        EntryKind::Project => "Project file",
        EntryKind::Other => "File",
    }
}

/// Props for [`Explorer`].
#[derive(Props, Clone, PartialEq)]
pub struct ExplorerProps {
    /// Org slug whose Files service to talk to.
    pub org: String,
    /// Where to start.
    pub start: Location,
    /// A slice the explorer is pinned to: navigation never rises above
    /// this path. Empty = the whole root (or the Drive path given).
    #[props(default)]
    pub floor: String,
    /// Root header (name + badges) to render above the listing.
    #[props(default)]
    pub root: Option<FileRootInfo>,
    /// Compact chrome for the note-embedded mount.
    #[props(default)]
    pub embedded: bool,
}

/// The browsing shell. Mounted by both the pane and the note widget.
#[component]
pub fn Explorer(props: ExplorerProps) -> Element {
    let org = props.org.clone();
    let floor = props.floor.clone();
    let embedded = props.embedded;
    let mut location = use_signal(|| props.start.clone());
    // The selected entry's name — the inspector's subject. Cleared on
    // any navigation or root swap (a selection belongs to one folder).
    let mut selected = use_signal(|| Option::<String>::None);
    let mut view = use_signal(|| {
        if embedded {
            ViewMode::List
        } else {
            ViewMode::Grid
        }
    });
    let mut inspector_open = use_signal(|| true);
    // The quick-preview overlay's subject (Space / double-click) — a
    // NAME, resolved against the live listing at render (a stored row
    // snapshot would go stale under checkpoints, like the selection
    // comment below says).
    let mut previewing = use_signal(|| Option::<String>::None);

    // Follow prop changes (the pane swaps roots underneath us).
    let start = props.start.clone();
    use_effect(use_reactive!(|start| {
        location.set(start);
        selected.set(None);
        previewing.set(None);
    }));

    let mut entries = {
        let org = org.clone();
        use_resource(move || {
            let org = org.clone();
            let here = location.read().clone();
            async move { fetch_entries(&org, &here).await }
        })
    };

    // Live: a checkpoint (or a new root) anywhere re-reads this
    // listing, so a save on another device lands here without a
    // refresh.
    {
        let org = org.clone();
        crate::use_files_events(
            move || org.clone(),
            move |event: FilesEvent| {
                let mut entries = entries;
                // Only events that can change browse() output restart
                // the listing. Naming / Project Version events write
                // vault pages and chain badges — the Inspector's own
                // stream handles those; restarting here double-fetched
                // for output that could not have moved.
                let touched = match &event {
                    FilesEvent::Checkpointed(info) => Some(info.root_id),
                    FilesEvent::Snapshotted(snap) => Some(snap.root_id),
                    FilesEvent::HydrationChanged(change) => Some(change.root_id),
                    // A new root can adopt loose files out from under a
                    // Drive listing, so it refreshes BOTH surfaces.
                    FilesEvent::RootCreated(root) => {
                        if location.peek().root_id().is_none() {
                            entries.restart();
                            return;
                        }
                        Some(root.id)
                    }
                    FilesEvent::VersionNamed(_)
                    | FilesEvent::VersionUnnamed(_)
                    | FilesEvent::ProjectVersionStarted(_)
                    | FilesEvent::ReviewCreated(_)
                    | FilesEvent::ReviewCommentAdded(_)
                    | FilesEvent::ReviewCommentDeleted(_) => return,
                };
                // Root browsing refreshes on its own root's traffic; a
                // Drive listing lives outside every root, and a
                // checkpoint inside one cannot move loose files.
                if location.peek().root_id() == touched {
                    entries.restart();
                }
            },
        );
    }

    let listing = entries.read_unchecked().clone();
    let here_root: Option<Uuid> = location.read().root_id();
    let here_base: String = location.read().path().to_string();
    let crumb_items = crumbs(&location.read().clone(), &floor);
    let at_floor = crumb_items.is_empty();
    // Embedded (note-widget) mounts stay a card; the pane mount fills
    // its parent edge to edge and scrolls internally.
    let shell_class = if embedded {
        "flex flex-col gap-2 rounded-lg border border-border/40 bg-card/40 p-3 outline-none"
    } else {
        "flex h-full min-h-0 flex-col gap-2 p-3 outline-none"
    };

    // The selected entry's record, resolved fresh from the listing so
    // badges (divergent, stub) are never stale.
    let selected_name = selected();
    let selected_entry: Option<BrowseEntry> = match (&listing, &selected_name) {
        (Some(Ok(rows)), Some(name)) => rows.iter().find(|e| &e.name == name).cloned(),
        _ => None,
    };
    // Same derivation for the preview: the overlay follows the live
    // row (and closes itself if the file vanishes under it).
    let preview_entry: Option<BrowseEntry> = match (&listing, previewing()) {
        (Some(Ok(rows)), Some(name)) => rows.iter().find(|e| e.name == name).cloned(),
        _ => None,
    };
    let selected_path = selected_entry.as_ref().map(|e| {
        if here_base.is_empty() {
            e.name.clone()
        } else {
            format!("{here_base}/{}", e.name)
        }
    });

    // One navigation closure shared by both views.
    let mut open_entry = {
        let mut location = location;
        move |entry: &BrowseEntry| {
            if entry.is_dir {
                let next = location.peek().child(&entry.name);
                location.set(next);
                selected.set(None);
            } else {
                previewing.set(Some(entry.name.clone()));
            }
        }
    };

    rsx! {
        div {
            class: shell_class,
            tabindex: 0,
            onkeydown: {
                move |evt: Event<KeyboardData>| {
                    let guard = entries.peek();
                    let Some(Ok(rows)) = &*guard else {
                        return;
                    };
                    match evt.data().key() {
                        Key::Escape => {
                            if previewing.peek().is_some() {
                                previewing.set(None);
                            } else {
                                selected.set(None);
                            }
                        }
                        Key::Enter => {
                            if let Some(name) = selected.peek().clone()
                                && let Some(entry) = rows.iter().find(|e| e.name == name).cloned()
                            {
                                evt.prevent_default();
                                drop(guard);
                                open_entry(&entry);
                            }
                        }
                        Key::Character(c) if c == " " => {
                            if let Some(name) = selected.peek().clone()
                                && let Some(entry) = rows.iter().find(|e| e.name == name)
                                && !entry.is_dir
                            {
                                evt.prevent_default();
                                let name = entry.name.clone();
                                drop(guard);
                                previewing.set(Some(name));
                            }
                        }
                        _ => {}
                    }
                }
            },
            if let Some(root) = &props.root {
                RootHeader {
                    org: org.clone(),
                    root: root.clone(),
                    slice: floor.clone(),
                }
            }
            // ── toolbar ─────────────────────────────────────────
            div { class: "flex items-center gap-1 text-xs text-muted-foreground flex-wrap",
                button {
                    class: "rounded px-1.5 py-0.5 hover:bg-muted/50 disabled:opacity-60",
                    disabled: at_floor,
                    onclick: {
                        let floor = floor.clone();
                        move |_| {
                            let next = location.peek().with_path(floor.clone());
                            location.set(next);
                            selected.set(None);
                        }
                    },
                    if floor.is_empty() { "Root" } else { "{floor}" }
                }
                for (label , path) in crumb_items {
                    span { "/" }
                    button {
                        class: "rounded px-1.5 py-0.5 hover:bg-muted/50",
                        onclick: move |_| {
                            let next = location.peek().with_path(path.clone());
                            location.set(next);
                            selected.set(None);
                        },
                        "{label}"
                    }
                }
                div { class: "ml-auto flex items-center gap-0.5",
                    button {
                        class: if view() == ViewMode::Grid { "flex h-6 w-6 items-center justify-center rounded text-indigo-400 bg-indigo-500/10" } else { "flex h-6 w-6 items-center justify-center rounded hover:bg-muted/50" },
                        title: "Grid view",
                        onclick: move |_| view.set(ViewMode::Grid),
                        LayoutGrid { size: 13 }
                    }
                    button {
                        class: if view() == ViewMode::List { "flex h-6 w-6 items-center justify-center rounded text-indigo-400 bg-indigo-500/10" } else { "flex h-6 w-6 items-center justify-center rounded hover:bg-muted/50" },
                        title: "List view",
                        onclick: move |_| view.set(ViewMode::List),
                        ListIcon { size: 13 }
                    }
                    if !embedded {
                        button {
                            class: if inspector_open() { "ml-1 flex h-6 w-6 items-center justify-center rounded text-indigo-400 bg-indigo-500/10" } else { "ml-1 flex h-6 w-6 items-center justify-center rounded hover:bg-muted/50" },
                            title: "Toggle inspector",
                            onclick: move |_| {
                                let open = *inspector_open.peek();
                                inspector_open.set(!open);
                            },
                            PanelRight { size: 13 }
                        }
                    }
                }
            }
            // ── body: listing + inspector ───────────────────────
            div { class: if embedded { "flex flex-col gap-2" } else { "flex min-h-0 flex-1 items-stretch gap-3" },
                div { class: if embedded { "min-w-0 flex-1" } else { "min-w-0 flex-1 overflow-y-auto" },
                    {match listing.clone() {
                        None => rsx! {
                            task_ui_core::states::LoadingState { rows: 3 }
                        },
                        Some(Err(e)) => rsx! {
                            task_ui_core::states::ErrorState {
                                message: e,
                                on_retry: move |()| entries.restart(),
                            }
                        },
                        Some(Ok(rows)) if rows.is_empty() => rsx! {
                            task_ui_core::states::EmptyState {
                                title: "Nothing here yet",
                                hint: "Files saved into this folder show up on the next Session checkpoint.",
                            }
                        },
                        Some(Ok(rows)) => rsx! {
                            if view() == ViewMode::Grid {
                                div { class: "grid gap-1", style: "grid-template-columns:repeat(auto-fill,minmax(108px,1fr))",
                                    for entry in rows.iter().cloned() {
                                        GridTile {
                                            key: "{entry.name}",
                                            entry: entry.clone(),
                                            selected: selected_name.as_deref() == Some(entry.name.as_str()),
                                            on_select: {
                                                let name = entry.name.clone();
                                                move |()| selected.set(Some(name.clone()))
                                            },
                                            on_open: {
                                                let entry = entry.clone();
                                                let mut open_entry = open_entry;
                                                move |()| open_entry(&entry)
                                            },
                                        }
                                    }
                                }
                            } else {
                                div { class: "flex flex-col divide-y divide-border/20",
                                    for entry in rows.iter().cloned() {
                                        ListRowEntry {
                                            key: "{entry.name}",
                                            entry: entry.clone(),
                                            selected: selected_name.as_deref() == Some(entry.name.as_str()),
                                            on_select: {
                                                let name = entry.name.clone();
                                                move |()| selected.set(Some(name.clone()))
                                            },
                                            on_open: {
                                                let entry = entry.clone();
                                                let mut open_entry = open_entry;
                                                move |()| open_entry(&entry)
                                            },
                                        }
                                    }
                                }
                            }
                        },
                    }}
                }
                if (embedded || inspector_open()) && selected_entry.is_some() {
                    div { class: if embedded { "w-full" } else { "w-80 shrink-0 overflow-y-auto border-l border-border/40 pl-3" },
                        Inspector {
                            // The key is the whole fix for stale
                            // inspection: a different subject is a
                            // different Inspector, never a reused one
                            // whose chain closure captured the old
                            // file's path.
                            key: "{selected_path.clone().unwrap_or_default()}",
                            org: org.clone(),
                            root_id: here_root,
                            entry: selected_entry.clone().expect("selected_entry is Some"),
                            path: selected_path.clone().unwrap_or_default(),
                            on_mutated: move |()| entries.restart(),
                            on_close: move |()| selected.set(None),
                        }
                    }
                }
            }
        }
        // ── quick preview (Space / double-click) ────────────────
        if let Some(entry) = preview_entry {
            QuickPreview {
                key: "{entry.name}",
                org: org.clone(),
                root_id: here_root,
                entry: entry.clone(),
                path: if here_base.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{here_base}/{}", entry.name)
                },
                on_close: move |()| previewing.set(None),
            }
        }
    }
}

/// One grid tile: big kind icon, two-line name, state dots.
#[component]
pub(crate) fn GridTile(
    entry: BrowseEntry,
    selected: bool,
    on_select: EventHandler<()>,
    on_open: EventHandler<()>,
) -> Element {
    let kind = entry_kind(&entry);
    rsx! {
        button {
            class: if selected {
                "flex flex-col items-center gap-1 rounded-lg bg-indigo-500/10 ring-1 ring-indigo-400/50 px-1.5 py-2 text-center"
            } else {
                "flex flex-col items-center gap-1 rounded-lg px-1.5 py-2 text-center hover:bg-muted/30"
            },
            onclick: move |_| on_select.call(()),
            ondoubleclick: move |_| on_open.call(()),
            div { class: if entry.stub { "flex h-12 items-center justify-center opacity-40" } else { "flex h-12 items-center justify-center" },
                {kind_icon(kind, 36)}
            }
            span {
                class: if entry.stub { "w-full text-xs leading-tight text-muted-foreground line-clamp-2 break-all" } else { "w-full text-xs leading-tight line-clamp-2 break-all" },
                "{entry.name}"
            }
            div { class: "flex items-center gap-1 h-2",
                if entry.divergent {
                    span { class: "h-1.5 w-1.5 rounded-full bg-destructive", title: "Divergent versions" }
                }
                if entry.stub {
                    span { class: "h-1.5 w-1.5 rounded-full bg-border", title: "Pointer stub — not resident here" }
                }
            }
        }
    }
}

/// One list row: icon, name, badges, size.
#[component]
fn ListRowEntry(
    entry: BrowseEntry,
    selected: bool,
    on_select: EventHandler<()>,
    on_open: EventHandler<()>,
) -> Element {
    let kind = entry_kind(&entry);
    rsx! {
        button {
            class: if selected {
                "flex w-full items-center gap-2 rounded-md bg-indigo-500/10 px-2 py-1.5 text-left text-sm"
            } else {
                "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted/20"
            },
            onclick: move |_| on_select.call(()),
            ondoubleclick: move |_| on_open.call(()),
            {kind_icon(kind, 15)}
            span { class: if entry.stub { "truncate text-muted-foreground" } else { "truncate" },
                "{entry.name}"
            }
            if entry.stub {
                Badge { variant: BadgeVariant::Outline, "Stub" }
            }
            if entry.divergent {
                Badge { variant: BadgeVariant::Destructive, "Divergent" }
            }
            span { class: "ml-auto shrink-0 tabular-nums text-xs text-muted-foreground",
                "{human_size(&entry)}"
            }
        }
    }
}

/// The inspector: preview, metadata, divergence resolution, versions.
#[component]
fn Inspector(
    org: String,
    /// `None` on the Drive, where loose files aren't versioned.
    root_id: Option<Uuid>,
    entry: BrowseEntry,
    /// Full root-relative path of the entry.
    path: String,
    on_mutated: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let kind = entry_kind(&entry);

    // The version chain — fetched only while a versioned file is
    // inspected (a chain is derived per file, ADR 0001; a listing must
    // not pay for it).
    let chain = {
        let org = org.clone();
        let path = path.clone();
        let is_file = !entry.is_dir;
        use_resource(move || {
            let (org, path) = (org.clone(), path.clone());
            async move {
                match (root_id, is_file) {
                    (Some(root), true) => Some(fetch_chain(&org, root, path).await),
                    _ => None,
                }
            }
        })
    };

    // A chain changes on checkpoint / naming events — keep it live.
    {
        let org = org.clone();
        crate::use_files_events(
            move || org.clone(),
            move |event: FilesEvent| {
                let mut chain = chain;
                let touched = match &event {
                    FilesEvent::Checkpointed(i) => Some(i.root_id),
                    FilesEvent::VersionNamed(v) | FilesEvent::VersionUnnamed(v) => Some(v.root_id),
                    _ => None,
                };
                if touched.is_some() && touched == root_id {
                    chain.restart();
                }
            },
        );
    }

    let versions: Option<Result<Vec<ChainEntry>, String>> =
        chain.read_unchecked().clone().flatten();

    rsx! {
        div { class: "flex flex-col gap-3 py-1",
            // ── header ──────────────────────────────────────────
            div { class: "flex items-start gap-2",
                {kind_icon(kind, 20)}
                div { class: "min-w-0 flex-1",
                    div { class: "text-sm font-medium break-all", "{entry.name}" }
                    div { class: "text-[11px] text-muted-foreground",
                        "{kind_label(kind)}"
                        if !entry.is_dir { " · {human_size(&entry)}" }
                    }
                }
                button {
                    class: "flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-muted/50",
                    title: "Close inspector",
                    onclick: move |_| on_close.call(()),
                    X { size: 13 }
                }
            }
            // ── preview ─────────────────────────────────────────
            if kind == EntryKind::Video && root_id.is_some() {
                crate::review::MiniPlayer {
                    org: org.clone(),
                    root_id: root_id.expect("checked"),
                    path: path.clone(),
                }
            }
            // ── state ───────────────────────────────────────────
            if entry.stub || entry.divergent {
                div { class: "flex items-center gap-1.5 flex-wrap",
                    if entry.stub {
                        Badge { variant: BadgeVariant::Outline, "Stub — not resident here" }
                    }
                    if entry.divergent {
                        Badge { variant: BadgeVariant::Destructive, "Divergent" }
                    }
                }
            }
            if entry.divergent && root_id.is_some() {
                DivergencePanel {
                    org: org.clone(),
                    root_id: root_id.expect("checked"),
                    path: path.clone(),
                    on_mutated,
                }
            }
            // ── details ─────────────────────────────────────────
            div { class: "flex flex-col gap-1 text-[11px] text-muted-foreground",
                div { class: "flex gap-2",
                    span { class: "w-10 shrink-0 uppercase tracking-wider", "Path" }
                    span { class: "break-all font-mono", "/{path}" }
                }
            }
            // ── versions ────────────────────────────────────────
            if entry.is_dir {
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "Folders aren't versioned — their files are."
                }
            } else if root_id.is_none() {
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "Loose files on the Drive aren't versioned."
                }
            } else {
                div { class: "flex flex-col gap-1.5",
                    span { class: "text-[11px] uppercase tracking-wider text-muted-foreground font-medium",
                        "Versions"
                    }
                    {match versions {
                        None => rsx! {
                            Text { variant: TextVariant::Muted, class: "text-xs", "Reading version chain…" }
                        },
                        Some(Err(e)) => rsx! {
                            div { class: "text-xs text-destructive", "No chain: {e}" }
                        },
                        Some(Ok(list)) if list.is_empty() => rsx! {
                            div { class: "text-xs text-muted-foreground",
                                "No saved versions yet — this file has never been checkpointed."
                            }
                        },
                        Some(Ok(list)) => rsx! {
                            for version in list.iter().cloned() {
                                ChainRow {
                                    key: "{version.commit_id}",
                                    org: org.clone(),
                                    root_id: root_id.expect("checked"),
                                    version,
                                    on_mutated,
                                }
                            }
                        },
                    }}
                }
            }
        }
    }
}

/// The quick-preview overlay: a video plays big; everything else shows
/// its identity card. `Escape` or the backdrop closes.
#[component]
fn QuickPreview(
    org: String,
    root_id: Option<Uuid>,
    entry: BrowseEntry,
    path: String,
    on_close: EventHandler<()>,
) -> Element {
    let kind = entry_kind(&entry);
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6",
            onclick: move |_| on_close.call(()),
            div {
                class: "w-full max-w-4xl",
                onclick: |evt| evt.stop_propagation(),
                if kind == EntryKind::Video && root_id.is_some() {
                    crate::review::MiniPlayer {
                        org: org.clone(),
                        root_id: root_id.expect("checked"),
                        path: path.clone(),
                    }
                } else {
                    div { class: "flex flex-col items-center gap-3 rounded-xl border border-border bg-card p-10",
                        {kind_icon(kind, 56)}
                        span { class: "text-sm font-medium break-all text-center", "{entry.name}" }
                        span { class: "text-xs text-muted-foreground",
                            "{kind_label(kind)}"
                            if !entry.is_dir { " · {human_size(&entry)}" }
                        }
                        Text { variant: TextVariant::Muted, class: "text-xs",
                            "No inline preview for this type yet."
                        }
                    }
                }
            }
        }
    }
}

// ── carried over from the flat explorer (unchanged behavior) ──────

/// The root's name, flavor, and Project Version badge, plus the slice
/// the explorer is pinned to (when it is) — and the Share action that
/// mints a link scoped to exactly this slice (issue #271).
#[component]
fn RootHeader(org: String, root: FileRootInfo, slice: String) -> Element {
    let toast = use_toast();
    let mut busy = use_signal(|| false);
    let share = {
        let org = org.clone();
        let slice = slice.clone();
        let root_id = root.id;
        move |_| {
            if *busy.peek() {
                return;
            }
            busy.set(true);
            let (org, slice) = (org.clone(), slice.clone());
            spawn(async move {
                let target = share_proto::ShareTarget::Slice {
                    root_id,
                    subpath: slice.clone(),
                };
                match mint_share_link(&org, target, None).await {
                    Ok(url) => {
                        copy_to_clipboard(&url);
                        toast.success(
                            "Share link minted".into(),
                            ToastOptions::new().description(format!("Copied: {url}")),
                        );
                    }
                    Err(e) => {
                        toast.error(
                            "Couldn't mint link".into(),
                            ToastOptions::new().description(e),
                        );
                    }
                }
                busy.set(false);
            });
        }
    };
    rsx! {
        div { class: "flex items-baseline gap-2 flex-wrap",
            span { class: "text-sm font-medium", "{root.name}" }
            if let Some(badge) = &root.project_version {
                Badge { variant: BadgeVariant::Secondary, "{project_version_label(badge)}" }
            }
            if !slice.is_empty() {
                Badge { variant: BadgeVariant::Outline, "slice: {slice}" }
            }
            span { class: "text-xs text-muted-foreground truncate", "{root.path}" }
            // Slice links serve bytes from the Media CAS — a software
            // root would mint a link where every download 404s, so the
            // affordance only exists where it works (the server refuses
            // the mint too).
            if root.flavor == files_proto::RootFlavor::Media {
                div { class: "ml-auto",
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        disabled: busy(),
                        on_click: share,
                        "Share…"
                    }
                }
            }
        }
    }
}

/// One checkpoint in a file's chain, with its kind distinguished
/// (Checkpoint / Named Version / save points) and Restore + Name
/// actions (issue #267).
#[component]
fn ChainRow(
    org: String,
    root_id: Uuid,
    version: ChainEntry,
    on_mutated: EventHandler<()>,
) -> Element {
    let mut naming = use_signal(|| false);
    let name_input = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let toast = use_toast();

    let commit = version.commit_id.clone();
    let vpath = version.path.clone();

    let do_restore = {
        let org = org.clone();
        let commit = commit.clone();
        let vpath = vpath.clone();
        move |_| {
            if *busy.peek() {
                return;
            }
            busy.set(true);
            let (org, commit, vpath) = (org.clone(), commit.clone(), vpath.clone());
            spawn(async move {
                match restore_file(&org, root_id, commit.clone(), vpath.clone()).await {
                    Ok(()) => {
                        toast.success(
                            "File restored".into(),
                            ToastOptions::new()
                                .description(format!("{vpath} from {}", short_id(&commit))),
                        );
                        on_mutated.call(());
                    }
                    Err(e) => {
                        toast.error("Restore failed".into(), ToastOptions::new().description(e));
                    }
                }
                busy.set(false);
            });
        }
    };

    let do_name = {
        let org = org.clone();
        let commit = commit.clone();
        move |_| {
            let name = name_input.peek().trim().to_string();
            if name.is_empty() || *busy.peek() {
                return;
            }
            busy.set(true);
            let (org, commit) = (org.clone(), commit.clone());
            let mut name_input = name_input;
            spawn(async move {
                match name_a_version(&org, root_id, commit.clone(), name.clone()).await {
                    Ok(()) => {
                        toast.success(
                            "Version named".into(),
                            ToastOptions::new().description(name),
                        );
                        naming.set(false);
                        name_input.set(String::new());
                        on_mutated.call(());
                    }
                    Err(e) => {
                        toast.error(
                            "Couldn't name version".into(),
                            ToastOptions::new().description(e),
                        );
                    }
                }
                busy.set(false);
            });
        }
    };

    rsx! {
        div { class: "flex flex-col gap-1 rounded-md border border-border/30 px-2 py-1.5",
            div { class: "flex items-center gap-2 text-xs text-muted-foreground tabular-nums flex-wrap",
                // Every chain entry is a Session checkpoint — the unit
                // the chain is built from.
                Badge { variant: BadgeVariant::Outline, "Checkpoint" }
                span { class: "font-mono", "{short_id(&version.commit_id)}" }
                if let Some(from) = &version.renamed_from {
                    Badge { variant: BadgeVariant::Outline, "renamed from {from}" }
                }
                // A checkpoint the Vault curates is a Named Version.
                for nm in version.names.iter() {
                    Badge { variant: BadgeVariant::Secondary, "★ {nm}" }
                }
                // Save points recorded during the session this checkpoint
                // closed (not versions themselves — display metadata).
                if !version.save_points.is_empty() {
                    Badge { variant: BadgeVariant::Outline,
                        "{version.save_points.len()} save point"
                        if version.save_points.len() != 1 { "s" }
                    }
                }
            }
            div { class: "flex items-center gap-1 justify-end",
                // A curated checkpoint can be handed out as a link
                // that resolves to exactly this change (issue #271).
                if !version.names.is_empty() {
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        disabled: busy(),
                        on_click: {
                            let org = org.clone();
                            let commit = version.commit_id.clone();
                            move |_| {
                                if *busy.peek() {
                                    return;
                                }
                                busy.set(true);
                                let (org, commit) = (org.clone(), commit.clone());
                                let toast = toast;
                                spawn(async move {
                                    match mint_named_version_link(&org, root_id, &commit).await {
                                        Ok(url) => {
                                            copy_to_clipboard(&url);
                                            toast.success(
                                                "Version link minted".into(),
                                                ToastOptions::new().description(format!("Copied: {url}")),
                                            );
                                        }
                                        Err(e) => {
                                            toast.error(
                                                "Couldn't mint link".into(),
                                                ToastOptions::new().description(e),
                                            );
                                        }
                                    }
                                    busy.set(false);
                                });
                            }
                        },
                        "Share"
                    }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: do_restore,
                    "Restore"
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: move |_| {
                        let next = !*naming.peek();
                        naming.set(next);
                    },
                    if naming() { "Cancel" } else { "Name…" }
                }
            }
            if naming() {
                div { class: "flex items-center gap-2",
                    Input {
                        value: name_input,
                        size: InputSize::Small,
                        placeholder: "Version name (e.g. Final Mix)".to_string(),
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Small,
                        disabled: busy(),
                        on_click: do_name,
                        "Save name"
                    }
                }
            }
        }
    }
}

/// The divergence-resolution panel (issue #267): both sides of a
/// divergent file, and the Pick-a-side / Keep-both choices that write
/// the merge checkpoint.
#[component]
fn DivergencePanel(
    org: String,
    root_id: Uuid,
    path: String,
    on_mutated: EventHandler<()>,
) -> Element {
    let mut busy = use_signal(|| false);
    let toast = use_toast();

    let info = {
        let org = org.clone();
        use_resource(move || {
            let org = org.clone();
            async move { fetch_divergences(&org, root_id).await }
        })
    };

    // Keep the sides live: a concurrent save/sync on this root changes
    // the divergent heads, and the `commit_id`s the Pick resolver uses
    // must not go stale under the panel.
    {
        let org = org.clone();
        crate::use_files_events(
            move || org.clone(),
            move |event: FilesEvent| {
                let mut info = info;
                let touched = match &event {
                    FilesEvent::Checkpointed(i) => Some(i.root_id),
                    FilesEvent::HydrationChanged(c) => Some(c.root_id),
                    _ => None,
                };
                if touched == Some(root_id) {
                    info.restart();
                }
            },
        );
    }

    // The resolver: a `DivergenceChoice`, run + toast + refresh.
    let resolve = {
        let org = org.clone();
        let path = path.clone();
        move |choice: DivergenceChoice, label: String| {
            if *busy.peek() {
                return;
            }
            busy.set(true);
            let (org, path) = (org.clone(), path.clone());
            spawn(async move {
                match resolve_a_divergence(&org, root_id, path.clone(), choice).await {
                    Ok(()) => {
                        toast.success(
                            "Divergence resolved".into(),
                            ToastOptions::new().description(format!("{path}: {label}")),
                        );
                        on_mutated.call(());
                    }
                    Err(e) => {
                        toast.error(
                            "Couldn't resolve".into(),
                            ToastOptions::new().description(e),
                        );
                    }
                }
                busy.set(false);
            });
        }
    };

    let state = info.read_unchecked().clone();
    let mine: Option<DivergenceInfo> = match &state {
        Some(Ok(list)) => list.iter().find(|d| d.path == path).cloned(),
        _ => None,
    };

    rsx! {
        div { class: "flex flex-col gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2",
            div { class: "flex items-center gap-2",
                Badge { variant: BadgeVariant::Destructive, "Divergent" }
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "Concurrent saves survive side by side. Keep one, or keep both."
                }
            }
            {match &state {
                None => rsx! {
                    Text { variant: TextVariant::Muted, class: "text-xs", "Reading both sides…" }
                },
                Some(Err(e)) => rsx! {
                    div { class: "text-xs text-destructive", "Couldn't read divergence: {e}" }
                },
                Some(Ok(_)) if mine.is_none() => rsx! {
                    // Resolved elsewhere between listing and open.
                    Text { variant: TextVariant::Muted, class: "text-xs", "No longer divergent." }
                },
                Some(Ok(_)) => {
                    let info = mine.clone().expect("mine is Some");
                    rsx! {
                        div { class: "flex flex-col gap-1",
                            for (i , side) in info.sides.iter().enumerate() {
                                div { class: "flex items-center gap-2 text-xs tabular-nums flex-wrap",
                                    Badge { variant: BadgeVariant::Outline,
                                        if i == 0 { "This device" } else { "Side {i + 1}" }
                                    }
                                    span { class: "font-mono text-muted-foreground",
                                        "{short_id(&side.commit_id)}"
                                    }
                                    match &side.file_id {
                                        Some(fid) => rsx! {
                                            span { class: "font-mono text-muted-foreground truncate",
                                                "{short_id(fid)}"
                                            }
                                        },
                                        None => rsx! {
                                            Badge { variant: BadgeVariant::Outline, "deleted here" }
                                        },
                                    }
                                    div { class: "ml-auto",
                                        Button {
                                            variant: ButtonVariant::Secondary,
                                            size: ButtonSize::Small,
                                            disabled: busy(),
                                            on_click: {
                                                let mut resolve = resolve.clone();
                                                let commit = side.commit_id.clone();
                                                move |_| {
                                                    resolve(
                                                        DivergenceChoice::Pick { commit_id: commit.clone() },
                                                        format!("kept {}", short_id(&commit)),
                                                    );
                                                }
                                            },
                                            "Keep this side"
                                        }
                                    }
                                }
                            }
                            div { class: "flex justify-end pt-1",
                                Button {
                                    variant: ButtonVariant::Outline,
                                    size: ButtonSize::Small,
                                    disabled: busy(),
                                    on_click: {
                                        let mut resolve = resolve.clone();
                                        move |_| resolve(DivergenceChoice::KeepBoth, "kept both".into())
                                    },
                                    "Keep both"
                                }
                            }
                        }
                    }
                },
            }}
        }
    }
}
