//! The Files explorer (issue #266) — one browser over the Files RPC
//! surface, mounted two ways:
//!
//! - **Standalone pane** ([`FilesPane`], the shell's `/files` route):
//!   the org's File Roots down the side, the selected root's live tree
//!   in the middle, and the **Drive** surface (loose files outside any
//!   root) as a sibling choice.
//! - **Note-embedded widget** ([`widgets`]): a `type: file-root` note
//!   *is* its explorer, and any other note embeds one with
//!   `experience: files` — narrowed to a **root slice** when the note
//!   names one.
//!
//! Both mount the same [`Explorer`]; the pane adds the root picker.
//!
//! **Badges come from live RPC data, never from local guesswork**
//! (glossary vocabulary throughout):
//!
//! - *Stub* — [`files_proto::BrowseEntry::stub`]: the version store
//!   tracks the path but the live tree doesn't hold it. Browsing a
//!   240 GB project never means downloading it.
//! - *Divergent* — [`files_proto::BrowseEntry::divergent`]: concurrent
//!   saves survive side by side; the badge rides the entry until
//!   someone resolves it (issue #267).
//! - *Project Version* — [`files_proto::FileRootInfo::project_version`]:
//!   the root's current lineage (its highest-numbered `ProjectVersion`
//!   Vault entity, issue #261), so "Project NEW final2" folders stay
//!   dead.
//! - *Versions* — a file's [`files_proto::ChainEntry`] count, fetched
//!   when a row is opened (the chain is derived per file, so it is a
//!   click, not a listing column).
//!
//! Live updates ride the service's `#[subscribe] fn events` stream: a
//! Session checkpoint taken anywhere — another device, the CLI, the
//! cadence engine — re-reads the listing in place, with no refresh.

/// The Review player (issue #270): proxy playback + timecode seek +
/// filmstrip scrub for an opened media file.
pub mod review;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use files_proto::{
    BrowseEntry, ChainEntry, DivergenceChoice, DivergenceInfo, FileRootInfo, FilesEvent,
    FilesServiceClient, FilesServiceStreamClient, ProjectVersion,
};
use task_ui_core::orgs::{OrgMeta, OrgSelection};
use task_widgets::{WidgetCtx, WidgetMatch, WidgetSpec, WidgetTarget};
use uuid::Uuid;

/// What to show where a root's local path goes.
///
/// A root this host holds the structure of has no path here, and a
/// blank line reads as a bug. Naming the state is also the only cue a
/// user gets that opening a file will reach for another host.
pub(crate) fn placement_label(root: &files_proto::model::FileRootInfo) -> String {
    root.path
        .clone()
        .unwrap_or_else(|| "structure only — content on another host".to_owned())
}

/// The note frontmatter `type:` that makes a note a File Root page.
const NOTE_TYPE: &str = "file-root";

// ── RPC ───────────────────────────────────────────────────────────

async fn client(org: &str) -> Result<FilesServiceClient, String> {
    task_ui_core::vox_clients::establish_for::<FilesServiceClient>(org).await
}

async fn fetch_roots(org: &str) -> Result<Vec<FileRootInfo>, String> {
    client(org)
        .await?
        .list_roots()
        .await
        .map_err(|e| e.to_string())
}

/// The adopted root named `name`, if any — the convention project
/// surfaces resolve their material through (a project's root is named
/// after the project). `None` covers both "not adopted" and "can't
/// ask", which callers treat the same: fall back, don't error.
pub async fn root_named(org: &str, name: &str) -> Option<FileRootInfo> {
    fetch_roots(org)
        .await
        .ok()?
        .into_iter()
        .find(|r| r.name == name)
}

async fn fetch_entries(org: &str, scope: &Location) -> Result<Vec<BrowseEntry>, String> {
    let c = client(org).await?;
    match scope {
        Location::Root { id, subpath } => c.browse(*id, subpath.clone()).await,
        Location::Drive { path } => c.drive_browse(path.clone()).await,
    }
    .map_err(|e| e.to_string())
}

async fn fetch_chain(org: &str, root: Uuid, path: String) -> Result<Vec<ChainEntry>, String> {
    client(org)
        .await?
        .chain(root, path)
        .await
        .map_err(|e| e.to_string())
}

// ── History & divergence mutations (issue #267) ───────────────────

async fn fetch_divergences(org: &str, root: Uuid) -> Result<Vec<DivergenceInfo>, String> {
    client(org)
        .await?
        .divergences(root)
        .await
        .map_err(|e| e.to_string())
}

/// Restore one file's state from a past checkpoint into the live tree
/// (the `copy_forward` verb). `path` is the file's path *in that commit*.
async fn restore_file(org: &str, root: Uuid, commit: String, path: String) -> Result<(), String> {
    client(org)
        .await?
        .copy_forward(root, commit, vec![path])
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Name a checkpoint — creates the Named Version Vault entity.
async fn name_a_version(org: &str, root: Uuid, commit: String, name: String) -> Result<(), String> {
    client(org)
        .await?
        .name_version(root, commit, name)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Resolve a divergent file — Pick a side or KeepBoth — writing the
/// merge checkpoint that carries the decision.
async fn resolve_a_divergence(
    org: &str,
    root: Uuid,
    path: String,
    choice: DivergenceChoice,
) -> Result<(), String> {
    client(org)
        .await?
        .resolve_divergence(root, path, choice)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ── Share links (issue #271) ──────────────────────────────────────

/// Mint a share link and hand back its URL. `capabilities: None` is
/// the view-only default (edited in the Links registry afterwards);
/// `Some` grants at mint time (the review flow's comment-on link).
pub(crate) async fn mint_share_link(
    org: &str,
    target: share_proto::ShareTarget,
    capabilities: Option<share_proto::ShareCapabilities>,
) -> Result<String, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<share_proto::ShareServiceClient>(org).await?;
    client
        .create_link(
            target,
            share_proto::NewShareLink {
                label: String::new(),
                capabilities,
                password: None,
                expires_unix: None,
            },
        )
        .await
        .map(|link| link.url)
        .map_err(|e| e.to_string())
}

/// Mint a link for the Named Version curated at `commit_id` — the chain
/// row knows the names, not the entity id, so resolve it first.
async fn mint_named_version_link(
    org: &str,
    root_id: Uuid,
    commit_id: &str,
) -> Result<String, String> {
    let c = client(org).await?;
    let named = c
        .list_named_versions(Some(root_id))
        .await
        .map_err(|e| e.to_string())?;
    let version = named
        .into_iter()
        .find(|v| v.commit_id == commit_id)
        .ok_or_else(|| "no Named Version at this checkpoint".to_string())?;
    mint_share_link(
        org,
        share_proto::ShareTarget::NamedVersion { id: version.id },
        None,
    )
    .await
}

/// Copy `text` to the clipboard (browser only; a no-op on native).
fn copy_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(win) = web_sys::window() {
            let _ = win.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = text;
}

// ── Scope ─────────────────────────────────────────────────────────

/// Where the explorer is currently looking. Root browsing and Drive
/// browsing are distinct surfaces (glossary), not two modes of one
/// call — they answer different RPCs and only root browsing carries
/// version-store badges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// Inside a File Root's live tree, at a root-relative subpath
    /// (empty = the root itself).
    Root { id: Uuid, subpath: String },
    /// Loose files outside any root, at an absolute path.
    Drive { path: String },
}

impl Location {
    fn root_id(&self) -> Option<Uuid> {
        match self {
            Self::Root { id, .. } => Some(*id),
            Self::Drive { .. } => None,
        }
    }

    /// The path as displayed / navigated: root-relative inside a root,
    /// absolute on Drive.
    fn path(&self) -> &str {
        match self {
            Self::Root { subpath, .. } => subpath,
            Self::Drive { path } => path,
        }
    }

    fn with_path(&self, path: String) -> Self {
        match self {
            Self::Root { id, .. } => Self::Root {
                id: *id,
                subpath: path,
            },
            Self::Drive { .. } => Self::Drive { path },
        }
    }

    /// Descend into `name`.
    fn child(&self, name: &str) -> Self {
        let joined = match self {
            Self::Root { subpath, .. } if subpath.is_empty() => name.to_owned(),
            Self::Root { subpath, .. } => format!("{subpath}/{name}"),
            Self::Drive { path } => format!("{}/{name}", path.trim_end_matches('/')),
        };
        self.with_path(joined)
    }
}

/// Path segments of `location` below `floor` (the slice the explorer
/// is pinned to), as (label, path) pairs — the breadcrumb's data.
fn crumbs(location: &Location, floor: &str) -> Vec<(String, String)> {
    let path = location.path();
    let rest = path.strip_prefix(floor).unwrap_or(path);
    let rest = rest.trim_matches('/');
    if rest.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut acc = floor.trim_end_matches('/').to_owned();
    for part in rest.split('/') {
        if acc.is_empty() {
            acc = part.to_owned();
        } else {
            acc = format!("{acc}/{part}");
        }
        out.push((part.to_owned(), acc.clone()));
    }
    out
}

/// A byte count for a listing row. Stubs have no live-tree size — that
/// absence is information (nothing to read locally), so it renders as a
/// dash rather than `0 B`.
fn human_size(entry: &BrowseEntry) -> String {
    let Some(bytes) = entry.size else {
        return "—".to_owned();
    };
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The root's Project Version badge label ("Project Version 2 · client
/// cut") — the projection of its current [`ProjectVersion`] entity
/// (issue #261).
fn project_version_label(version: &ProjectVersion) -> String {
    match &version.label {
        Some(label) if !label.is_empty() => {
            format!("Project Version {} · {label}", version.number)
        }
        _ => format!("Project Version {}", version.number),
    }
}

/// Subscribe to the org's `FilesEvent` stream — the ONE establishment
/// seam for every live surface in this crate. Six hand-rolled copies
/// of this block drifted before; a change to establishment/retry now
/// lands everywhere at once. `org` is read fresh on every
/// (re)establish, so reconnects follow org switches.
pub(crate) fn use_files_events(
    org: impl Fn() -> String + Clone + 'static,
    on_event: impl Fn(FilesEvent) + Clone + 'static,
) {
    architect::use_stream(
        move |tx| {
            let slug = org();
            async move {
                if slug.is_empty() {
                    return false;
                }
                let Ok(stream) =
                    task_ui_core::vox_clients::establish_for::<FilesServiceStreamClient>(&slug)
                        .await
                else {
                    return false;
                };
                stream.events(tx).await.is_ok()
            }
        },
        on_event,
    );
}

// ── The explorer ── the Spacedrive-style shell lives in `explorer` ──

pub mod explorer;
pub mod tree;
pub use explorer::{Explorer, ExplorerProps};

/// First 12 hex chars of a commit id — enough to identify a saved state
/// on screen.
pub(crate) fn short_id(commit_id: &str) -> String {
    commit_id.chars().take(12).collect()
}

// ── The standalone pane ───────────────────────────────────────────

/// What the pane is showing. Three states, not `Option<Uuid>`: "nothing
/// chosen yet" and "the user chose Drive" are different answers, and
/// collapsing them makes every roots refresh look like a fresh mount.
///
/// Shared as `Signal<Selection>` context provided by the app shell:
/// on the Files page the shell's sidebar column IS the Files sidebar
/// ([`FilesSidebar`]), and it drives the same selection the pane
/// renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// Before the roots list has landed — the only state the
    /// auto-select effect may act on.
    #[default]
    Unset,
    /// One area of the org tree (issue #304) — the unified namespace
    /// the OS mount serves too.
    Area(tree::TreeArea),
    /// The Drive surface (loose files outside any root).
    Drive,
    Root(Uuid),
}

/// The shell-column sidebar for the Files page: the org's roots and
/// the Drive surface. Replaces the vault explorer while Files is the
/// open view — the whole screen belongs to file management.
#[component]
pub fn FilesSidebar() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let org =
        use_memo(move || task_ui_core::orgs::active_slug(&selection.read(), &org_list.read()));
    let mut selected = use_context::<Signal<Selection>>();

    let mut roots = use_resource(move || async move {
        let slug = org();
        if slug.is_empty() {
            return Ok(Vec::new());
        }
        fetch_roots(&slug).await
    });
    use_files_events(
        move || org(),
        move |event: FilesEvent| {
            let mut roots = roots;
            if matches!(event, FilesEvent::RootCreated(_)) {
                roots.restart();
            }
        },
    );

    let rows = roots.read_unchecked().clone();
    let known: Vec<FileRootInfo> = match &rows {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "flex h-full min-h-0 flex-col gap-1 overflow-y-auto p-2",
            span { class: "px-2 pt-1 pb-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground",
                "Files"
            }
            // The org tree: the same namespace an OS mount shows.
            for area in tree::TreeArea::ALL {
                button {
                    key: "{area.as_str()}",
                    class: if *selected.read() == Selection::Area(area) {
                        "rounded-md bg-indigo-500/10 px-2.5 py-1.5 text-left text-sm ring-1 ring-indigo-400/40"
                    } else {
                        "rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted/30"
                    },
                    onclick: move |_| selected.set(Selection::Area(area)),
                    "{area.as_str()}"
                }
            }
            span { class: "px-2 pt-3 pb-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground",
                "Roots"
            }
            if let Some(Err(e)) = &rows {
                task_ui_core::states::ErrorState {
                    message: e.clone(),
                    title: "Couldn't list File Roots",
                    on_retry: move |()| roots.restart(),
                }
            }
            for root in known.iter().cloned() {
                button {
                    key: "{root.id}",
                    class: if *selected.read() == Selection::Root(root.id) {
                        "rounded-md bg-indigo-500/10 px-2.5 py-1.5 text-left text-sm ring-1 ring-indigo-400/40"
                    } else {
                        "rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted/30"
                    },
                    onclick: move |_| selected.set(Selection::Root(root.id)),
                    div { class: "flex items-center gap-2",
                        span { class: "truncate", "{root.name}" }
                        if let Some(badge) = &root.project_version {
                            Badge { variant: BadgeVariant::Secondary, "v{badge.number}" }
                        }
                    }
                    div { class: "truncate text-xs text-muted-foreground", "{placement_label(&root)}" }
                }
            }
            button {
                class: if *selected.read() == Selection::Drive {
                    "rounded-md bg-indigo-500/10 px-2.5 py-1.5 text-left text-sm ring-1 ring-indigo-400/40"
                } else {
                    "rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted/30"
                },
                onclick: move |_| selected.set(Selection::Drive),
                div { class: "flex items-center gap-2",
                    span { "Drive" }
                    Badge { variant: BadgeVariant::Outline, "loose files" }
                }
                div { class: "text-xs text-muted-foreground", "Outside any root" }
            }
        }
    }
}

/// The shell's `/files` pane: the org's roots down the side, the
/// selected root (or Drive) in the middle.
#[component]
pub fn FilesPane() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let org =
        use_memo(move || task_ui_core::orgs::active_slug(&selection.read(), &org_list.read()));

    let mut roots = use_resource(move || async move {
        let slug = org();
        if slug.is_empty() {
            return Ok(Vec::new());
        }
        fetch_roots(&slug).await
    });

    // A new root anywhere joins the picker without a refresh.
    use_files_events(
        move || org(),
        move |event: FilesEvent| {
            let mut roots = roots;
            if matches!(event, FilesEvent::RootCreated(_)) {
                roots.restart();
            }
        },
    );

    // The shell provides the selection; the Files sidebar (the shell
    // column) writes it. The local signal is ALWAYS created (hooks
    // must run unconditionally) and only used when no shell context
    // exists (tests, previews).
    let local = use_signal(|| Selection::Unset);
    let mut selected = try_use_context::<Signal<Selection>>().unwrap_or(local);
    let rows = roots.read_unchecked().clone();
    let known: Vec<FileRootInfo> = match &rows {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    // Land on the first root once the list arrives — ONLY from
    // `Unset`. The effect re-runs on every `roots` completion (a
    // `RootCreated` anywhere restarts it), so a two-state selection
    // that spelled Drive as "nothing selected" would yank a user off
    // Drive and drop the path they had typed (PR #288 review).
    use_effect(move || {
        if *selected.peek() != Selection::Unset {
            return;
        }
        if let Some(Ok(list)) = &*roots.read() {
            if let Some(first) = list.first() {
                selected.set(Selection::Root(first.id));
            }
        }
    });

    let selection = selected();
    let active = match selection {
        // An area selection browses the tree; no root is active.
        Selection::Area(_) => None,
        Selection::Root(id) => known.iter().find(|r| r.id == id).cloned(),
        Selection::Unset | Selection::Drive => None,
    };
    // A selected root that has vanished from the list (deleted
    // elsewhere) leaves the sidebar unhighlighted; say so rather than
    // silently showing Drive.
    let missing_root = matches!(selection, Selection::Root(_)) && active.is_none();

    rsx! {
        // Full-page experience: the pane owns the whole main area —
        // roots down a left rail, the explorer filling the rest. No
        // page heading, no content card, no dead margins.
        div { class: "flex h-full min-h-0 w-full",
            // ── roots rail (mobile fallback) ────────────────────
            // At md+ the SHELL's sidebar column is the Files sidebar
            // ([`FilesSidebar`]); this inline rail only exists below
            // md, where that column is hidden.
            div { class: "flex w-60 shrink-0 flex-col gap-1 overflow-y-auto border-r border-border/40 p-2 md:hidden",
                span { class: "px-2 pt-1 pb-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground",
                    "Files"
                }
                if let Some(Err(e)) = &rows {
                    task_ui_core::states::ErrorState {
                        message: e.clone(),
                        title: "Couldn't list File Roots",
                        on_retry: move |()| roots.restart(),
                    }
                }
                for root in known.iter().cloned() {
                    button {
                        key: "{root.id}",
                        class: if selection == Selection::Root(root.id) {
                            "rounded-md bg-indigo-500/10 px-2.5 py-1.5 text-left text-sm ring-1 ring-indigo-400/40"
                        } else {
                            "rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted/30"
                        },
                        onclick: move |_| selected.set(Selection::Root(root.id)),
                        div { class: "flex items-center gap-2",
                            span { class: "truncate", "{root.name}" }
                            if let Some(badge) = &root.project_version {
                                Badge { variant: BadgeVariant::Secondary, "v{badge.number}" }
                            }
                        }
                        div { class: "truncate text-xs text-muted-foreground", "{placement_label(&root)}" }
                    }
                }
                button {
                    class: if selection == Selection::Drive {
                        "rounded-md bg-indigo-500/10 px-2.5 py-1.5 text-left text-sm ring-1 ring-indigo-400/40"
                    } else {
                        "rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted/30"
                    },
                    onclick: move |_| selected.set(Selection::Drive),
                    div { class: "flex items-center gap-2",
                        span { "Drive" }
                        Badge { variant: BadgeVariant::Outline, "loose files" }
                    }
                    div { class: "text-xs text-muted-foreground", "Outside any root" }
                }
            }
            // ── the explorer, filling everything else ───────────
            div { class: "flex min-h-0 min-w-0 flex-1 flex-col",
                {match (rows.is_none(), active) {
                    (true, _) => rsx! {
                        div { class: "p-4",
                            task_ui_core::states::LoadingState { rows: 3 }
                        }
                    },
                    _ if matches!(selection, Selection::Area(_)) => {
                        let Selection::Area(area) = selection else {
                            unreachable!()
                        };
                        rsx! {
                            tree::TreeExplorer {
                                key: "{org()}:{area.as_str()}",
                                org: org(),
                                area,
                            }
                        }
                    }
                    (false, Some(root)) => rsx! {
                        Explorer {
                            key: "{org()}:{root.id}",
                            org: org(),
                            start: Location::Root { id: root.id, subpath: String::new() },
                            root: root.clone(),
                        }
                    },
                    (false, None) if missing_root => rsx! {
                        div { class: "p-4",
                            task_ui_core::states::EmptyState {
                                title: "That File Root is gone",
                                hint: "It is no longer in this org's roots — pick another, or browse Drive.",
                            }
                        }
                    },
                    (false, None) => rsx! {
                        DrivePane { key: "{org()}", org: org() }
                    },
                }}
            }
        }
    }
}

/// Drive browsing: loose files outside any root. The path is the user's
/// to type — the service confines it to the org's own files area, so an
/// out-of-bounds path comes back as an error rather than a listing.
#[component]
fn DrivePane(org: String) -> Element {
    let path = use_signal(String::new);
    let mut submitted = use_signal(String::new);
    rsx! {
        div { class: "flex h-full min-h-0 flex-col gap-3 p-3",
            div { class: "flex items-center gap-2",
                Input { value: path, placeholder: "Path to browse" }
                Button {
                    on_click: move |_| submitted.set(path()),
                    "Browse"
                }
            }
            if submitted().is_empty() {
                Text {
                    variant: TextVariant::Muted,
                    "Give a path to browse loose files. Everything outside a File Root is unversioned — make it a root to start versioning it."
                }
            } else {
                Explorer {
                    key: "{org}:{submitted()}",
                    org: org.clone(),
                    start: Location::Drive { path: submitted() },
                    floor: submitted(),
                }
            }
        }
    }
}

// ── The note-embedded widget ──────────────────────────────────────

/// The Files widget provider. Two ways a note embeds an explorer, both
/// scoped by the note's own `root:` / `slice:` frontmatter (glossary
/// "Root slice" — a reference to (root, subpath)):
///
/// - `type: file-root` — the note IS a File Root page.
/// - `experience: files` on any note — an explicit opt-in, so a project
///   note can carry the stems slice of its root inline.
///
/// The registry renders block widgets for note claims only
/// (`WidgetMatch::EmbedType` contributes decorations and href handling,
/// which are pure functions of the document and so can't carry live RPC
/// data) — hence the `experience:` opt-in rather than an
/// `![[Root note]]` embed claim.
///
/// Registered at the app root, like every other widget provider.
#[must_use]
pub fn widgets() -> Vec<WidgetSpec> {
    vec![
        WidgetSpec::new(
            "files.root",
            vec![
                // A `type: file-root` note IS its explorer…
                WidgetMatch::NoteType(NOTE_TYPE),
                // …and any other note opts in with `experience: files`
                // (a project note embedding the stems slice of its
                // root, say). Both read the same `root:` / `slice:`
                // frontmatter.
                WidgetMatch::NoteExperience("files"),
            ],
        )
        .render(|ctx| rsx! { FileRootNoteWidget { ctx } })
        .plugin("files"),
    ]
}

/// A File Root note's frontmatter scope: which root, and which slice of
/// it. `root:` accepts a root id or a root name; `slice:` is a
/// root-relative path (glossary "Slice" — a subtree of a root, the unit
/// selective sync and share links narrow to).
#[derive(Clone, Debug, PartialEq, Eq)]
struct NoteScope {
    root: String,
    slice: String,
}

fn scope_from_frontmatter(doc: &str) -> Option<NoteScope> {
    let scalar = |key: &str| {
        task_ui_core::frontmatter::frontmatter_value(doc, key)
            .map(|v| v.trim().trim_matches(['"', '\'']).trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Some(NoteScope {
        root: scalar("root")?,
        slice: scalar("slice")
            .unwrap_or_default()
            .trim_matches('/')
            .to_owned(),
    })
}

/// Resolve a note's `root:` — a root id, or a root name — against the
/// org's roots.
fn resolve_root(roots: &[FileRootInfo], reference: &str) -> Option<FileRootInfo> {
    if let Ok(id) = reference.parse::<Uuid>() {
        return roots.iter().find(|r| r.id == id).cloned();
    }
    roots
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(reference))
        .cloned()
}

/// The note-mounted explorer. The scope comes from the note's own
/// frontmatter (`root:` + optional `slice:`).
#[component]
fn FileRootNoteWidget(ctx: WidgetCtx) -> Element {
    let org = ctx.org.clone();
    let source = match &ctx.target {
        WidgetTarget::Note { .. } => Some((ctx.doc)()),
        // Defensive: the registry only mounts block widgets for note
        // claims today, but an embed claim would carry its target's
        // content (fetched lazily — a miss renders as "loading").
        WidgetTarget::Embed { target, .. } => (ctx.resolve)(target).and_then(|t| t.content),
        WidgetTarget::Fence { .. } => None,
    };
    let scope = source.as_deref().and_then(scope_from_frontmatter);

    let roots = {
        let org = org.clone();
        use_resource(move || {
            let org = org.clone();
            async move { fetch_roots(&org).await }
        })
    };

    let Some(scope) = scope else {
        return rsx! {
            div { class: "rounded-lg border border-border/40 bg-card/40 p-3 text-sm text-muted-foreground",
                "This File Root note names no root yet — add `root: <name or id>` (and an optional `slice:`) to its frontmatter."
            }
        };
    };

    let resolved = match &*roots.read_unchecked() {
        Some(Ok(list)) => Some(resolve_root(list, &scope.root)),
        Some(Err(_)) => Some(None),
        None => None,
    };

    match resolved {
        None => rsx! {
            task_ui_core::states::LoadingState { rows: 1 }
        },
        Some(None) => rsx! {
            task_ui_core::states::EmptyState {
                title: format!("No File Root named “{}”", scope.root),
                hint: "Point this note's `root:` at a File Root in this org — by name or by id.",
            }
        },
        Some(Some(root)) => rsx! {
            Explorer {
                org: org.clone(),
                start: Location::Root { id: root.id, subpath: scope.slice.clone() },
                floor: scope.slice.clone(),
                root: root.clone(),
                embedded: true,
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: Option<u64>) -> BrowseEntry {
        BrowseEntry {
            name: name.to_owned(),
            is_dir,
            size,
            stub: false,
            divergent: false,
        }
    }

    #[test]
    fn descending_builds_root_relative_and_absolute_paths() {
        let id = Uuid::new_v4();
        let root = Location::Root {
            id,
            subpath: String::new(),
        };
        assert_eq!(root.child("stems").path(), "stems");
        assert_eq!(
            root.child("stems").child("kick.wav").path(),
            "stems/kick.wav"
        );
        let drive = Location::Drive {
            path: "/srv/files/".to_owned(),
        };
        assert_eq!(drive.child("loose.wav").path(), "/srv/files/loose.wav");
    }

    #[test]
    fn breadcrumbs_never_rise_above_the_pinned_slice() {
        let id = Uuid::new_v4();
        let here = Location::Root {
            id,
            subpath: "stems/gtr/takes".to_owned(),
        };
        let labels = crumbs(&here, "stems");
        assert_eq!(
            labels,
            vec![
                ("gtr".to_owned(), "stems/gtr".to_owned()),
                ("takes".to_owned(), "stems/gtr/takes".to_owned()),
            ],
            "a slice-scoped embed can't navigate out of its slice"
        );
        // At the slice itself there is nothing above to show.
        let at_floor = Location::Root {
            id,
            subpath: "stems".to_owned(),
        };
        assert!(crumbs(&at_floor, "stems").is_empty());
    }

    #[test]
    fn stub_rows_show_no_live_tree_size() {
        assert_eq!(human_size(&entry("mix.wav", false, Some(2048))), "2.0 KB");
        assert_eq!(human_size(&entry("notes.txt", false, Some(12))), "12 B");
        assert_eq!(human_size(&entry("stems", true, None)), "—");
        let mut stub = entry("cut.mov", false, None);
        stub.stub = true;
        assert_eq!(human_size(&stub), "—");
    }

    fn project_version(number: u32, label: Option<&str>) -> ProjectVersion {
        ProjectVersion {
            id: Uuid::new_v4(),
            path: "Files/Project Versions/v2.md".to_owned(),
            root_id: Uuid::new_v4(),
            number,
            label: label.map(str::to_owned),
            change_id: "abc".to_owned(),
            commit_id: "def".to_owned(),
            started_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn project_version_badge_reads_as_lineage() {
        assert_eq!(
            project_version_label(&project_version(2, Some("client cut"))),
            "Project Version 2 · client cut"
        );
        assert_eq!(
            project_version_label(&project_version(3, None)),
            "Project Version 3"
        );
    }

    #[test]
    fn note_scope_comes_from_frontmatter() {
        let doc = "---\ntype: file-root\nroot: \"El Artisa\"\nslice: stems/gtr/\n---\n# Notes\n";
        let scope = scope_from_frontmatter(doc).expect("scope");
        assert_eq!(scope.root, "El Artisa");
        assert_eq!(scope.slice, "stems/gtr", "slice is a clean relative path");

        // No slice = the whole root.
        let whole =
            scope_from_frontmatter("---\ntype: file-root\nroot: Mix\n---\n").expect("scope");
        assert_eq!(whole.slice, "");

        // A note with no `root:` claims nothing.
        assert!(scope_from_frontmatter("---\ntype: file-root\n---\n").is_none());
    }

    #[test]
    fn root_reference_resolves_by_id_or_name() {
        let id = Uuid::new_v4();
        let roots = vec![FileRootInfo {
            id,
            name: "El Artisa".to_owned(),
            path: Some("/srv/files/el-artisa".to_owned()),
            flavor: files_proto::RootFlavor::Media,
            created_at: chrono::Utc::now(),
            project_version: None,
        }];
        assert_eq!(
            resolve_root(&roots, &id.to_string()).map(|r| r.id),
            Some(id)
        );
        assert_eq!(resolve_root(&roots, "el artisa").map(|r| r.id), Some(id));
        assert!(resolve_root(&roots, "Nope").is_none());
    }
}
