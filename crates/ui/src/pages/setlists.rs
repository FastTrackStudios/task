//! `/setlists` — every setlist the person can see, grouped by the
//! project that holds it, and `/setlists/:org/:id`, one setlist's songs
//! in order.
//!
//! A setlist is a collection (`CollectionService`) whose kind is
//! `setlist` or `songlist` — the words the apps that make them chose;
//! nothing here knows more about either. A project holds its setlists
//! with links in its org's graph, `project:<dir>` → `collection:<id>`
//! (`Down`), the shape `attach_setlist` makes, so an album holds its
//! running order and a weekly series one setlist a week. A song names
//! its project the same way: a `Down` or `Related` link between
//! `project:<dir>` and `song:<slug>`, in either direction.
//!
//! The list honours the org switcher like `/projects`: "All
//! organizations" fans out over every org of mine, and each row then
//! says whose it is. Read-only: setlists are edited in the app that
//! made them.

use std::collections::HashMap;

use architect_ui::lucide_dioxus::{ChevronRight, Music};
use architect_ui::prelude::*;
use collection_proto::{Collection, CollectionServiceClient};
use dioxus::prelude::*;
use links_proto::{LinksServiceClient, NodeKind, NodeRef, Relation, TypedLink};
use project_proto::ProjectInfo;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;

/// The collection kinds this page lists.
const SETLIST_KINDS: &[&str] = &["setlist", "songlist"];

/// Is a collection of this kind a setlist?
fn is_setlist_kind(kind: &str) -> bool {
    SETLIST_KINDS.contains(&kind)
}

// ── Pure shape ──────────────────────────────────────────────────────

/// A project's address in the links graph: the directory its page sits
/// in on the Projects tier — `crescendum-album/project.md` →
/// `crescendum-album`. A legacy flat page (`Projects/foo.md`) goes by
/// its path without the extension.
fn project_dir(path: &str) -> String {
    let path = path.trim().trim_matches('/');
    let dir = path
        .strip_suffix(org_proto::PROJECT_PAGE)
        .and_then(|d| d.strip_suffix('/'))
        .or_else(|| path.strip_suffix(".md"))
        .unwrap_or(path);
    dir.to_owned()
}

/// A slug read aloud: `cinematic-intro` → `Cinematic Intro`. What a
/// song with no published title is called, rather than its raw id.
fn humanise_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().chain(c).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an item is called: the title its publisher gave it, else its id
/// read aloud.
fn item_title(node: &NodeRef, published: &str) -> String {
    if published.trim().is_empty() {
        humanise_slug(&node.id)
    } else {
        published.trim().to_owned()
    }
}

/// The projects (by directory) that hold `collection`: each local
/// `project:` node with a `Down` link to it.
fn holders(links: &[TypedLink], collection: &NodeRef) -> Vec<String> {
    let mut dirs: Vec<String> = links
        .iter()
        .filter(|l| {
            l.target == *collection
                && l.relation == Relation::Down
                && l.source.kind == NodeKind::Project
                && l.source.is_local()
        })
        .map(|l| l.source.id.clone())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// The projects (by directory) a song belongs to: a `Down` or `Related`
/// link between a local `project:` node and the song, either way round.
fn song_projects(links: &[TypedLink], song: &NodeRef) -> Vec<String> {
    let mut dirs: Vec<String> = links
        .iter()
        .filter(|l| matches!(l.relation, Relation::Down | Relation::Related))
        .filter_map(|l| {
            let other = if l.target == *song {
                &l.source
            } else if l.source == *song {
                &l.target
            } else {
                return None;
            };
            (other.kind == NodeKind::Project && other.is_local()).then(|| other.id.clone())
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// One setlist as the list shows it.
#[derive(Clone, Debug, PartialEq)]
struct SetlistRow {
    org: String,
    id: String,
    title: String,
    kind: String,
    songs: usize,
    /// The directories of the projects that hold it.
    held_by: Vec<String>,
}

/// A project as a group heading: found in the project list, or only
/// known by the directory a link names.
#[derive(Clone, Debug, PartialEq)]
struct ProjectRef {
    org: String,
    /// `None` when no listed project lives at the linked directory.
    id: Option<Uuid>,
    title: String,
    form: Option<&'static str>,
}

/// The listed projects, by `(org, directory)`.
type ProjectIndex = HashMap<(String, String), ProjectRef>;

fn project_index<'a>(rows: impl IntoIterator<Item = (&'a str, &'a ProjectInfo)>) -> ProjectIndex {
    rows.into_iter()
        .map(|(org, p)| {
            (
                (org.to_owned(), project_dir(&p.path)),
                ProjectRef {
                    org: org.to_owned(),
                    id: Some(p.id),
                    title: p.title.clone(),
                    form: p.form.map(project_proto::Form::as_str),
                },
            )
        })
        .collect()
}

/// The project at `dir` in `org`, or a heading made from the directory.
fn project_at(index: &ProjectIndex, org: &str, dir: &str) -> ProjectRef {
    index
        .get(&(org.to_owned(), dir.to_owned()))
        .cloned()
        .unwrap_or_else(|| ProjectRef {
            org: org.to_owned(),
            id: None,
            title: humanise_slug(dir.rsplit('/').next().unwrap_or(dir)),
            form: None,
        })
}

/// One heading of the list and the setlists under it.
#[derive(Clone, Debug, PartialEq)]
struct SetlistGroup {
    /// `None` is "Not in a project".
    project: Option<ProjectRef>,
    setlists: Vec<SetlistRow>,
}

/// Case-folded, for ordering by what a person reads.
fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// The list's shape: a group per project that holds a setlist, by
/// project title, then the setlists no project holds. A setlist two
/// projects hold shows under both. Setlists order by title — a series'
/// dated titles fall into date order — then org.
fn group_by_project(rows: &[SetlistRow], index: &ProjectIndex) -> Vec<SetlistGroup> {
    let mut by_project: HashMap<(String, String), SetlistGroup> = HashMap::new();
    let mut loose: Vec<SetlistRow> = Vec::new();
    for row in rows {
        if row.held_by.is_empty() {
            loose.push(row.clone());
            continue;
        }
        for dir in &row.held_by {
            by_project
                .entry((row.org.clone(), dir.clone()))
                .or_insert_with(|| SetlistGroup {
                    project: Some(project_at(index, &row.org, dir)),
                    setlists: Vec::new(),
                })
                .setlists
                .push(row.clone());
        }
    }
    let by_title = |a: &SetlistRow, b: &SetlistRow| {
        fold(&a.title)
            .cmp(&fold(&b.title))
            .then_with(|| a.org.cmp(&b.org))
    };
    let mut groups: Vec<SetlistGroup> = by_project.into_values().collect();
    groups.sort_by_cached_key(|g| g.project.as_ref().map(|p| (fold(&p.title), p.org.clone())));
    for g in &mut groups {
        g.setlists.sort_by(by_title);
    }
    if !loose.is_empty() {
        loose.sort_by(by_title);
        groups.push(SetlistGroup {
            project: None,
            setlists: loose,
        });
    }
    groups
}

/// An org's display name, falling back to its slug.
fn org_name(orgs: &[OrgMeta], slug: &str) -> String {
    orgs.iter()
        .find(|o| o.slug == slug)
        .map_or_else(|| slug.to_owned(), |o| o.name.clone())
}

// ── Fetching ────────────────────────────────────────────────────────

/// Every link touching `node`, or none when the graph cannot be read —
/// a missing link is a missing heading, not a failed page.
async fn links_of(links: &LinksServiceClient, node: NodeRef) -> Vec<TypedLink> {
    links.links_for(node).await.unwrap_or_default()
}

/// One org's setlists, each with the projects that hold it.
async fn org_setlists(slug: String) -> Result<Vec<SetlistRow>, String> {
    let collections = crate::vox_clients::establish_for::<CollectionServiceClient>(&slug).await?;
    let links = crate::vox_clients::establish_for::<LinksServiceClient>(&slug).await?;
    let all = collections
        .list(slug.clone(), None)
        .await
        .map_err(|e| format!("{slug}: collections: {e:?}"))?;
    let setlists: Vec<Collection> = all
        .into_iter()
        .filter(|c| is_setlist_kind(c.kind.as_str()))
        .collect();
    let rows = setlists.into_iter().map(|c| {
        let links = &links;
        let slug = &slug;
        async move {
            let node = NodeRef::new(NodeKind::Collection, c.id.clone());
            let held_by = holders(&links_of(links, node.clone()).await, &node);
            SetlistRow {
                org: slug.clone(),
                id: c.id,
                title: c.title,
                kind: c.kind.as_str().to_owned(),
                songs: c.items.len(),
                held_by,
            }
        }
    });
    Ok(futures_util::future::join_all(rows).await)
}

/// Setlists across the selected orgs. An org that answers nothing (no
/// collection store, or unreachable) is skipped; an error only shows
/// when no org answered.
async fn fetch_setlists(slugs: Vec<String>) -> Result<Vec<SetlistRow>, String> {
    let per_org = futures_util::future::join_all(slugs.into_iter().map(org_setlists)).await;
    task_ui_core::feeds::collect(per_org)
}

/// One song of a setlist, resolved.
#[derive(Clone, Debug, PartialEq)]
struct SongRow {
    token: String,
    title: String,
    /// The directories of the projects it belongs to.
    projects: Vec<String>,
}

/// One setlist, its songs resolved and the projects that hold it.
#[derive(Clone, Debug, PartialEq)]
struct SetlistDetail {
    title: String,
    kind: String,
    held_by: Vec<String>,
    songs: Vec<SongRow>,
}

async fn fetch_setlist(org: String, id: String) -> Result<Option<SetlistDetail>, String> {
    let collections = crate::vox_clients::establish_for::<CollectionServiceClient>(&org).await?;
    let links = crate::vox_clients::establish_for::<LinksServiceClient>(&org).await?;
    let Some(c) = collections
        .get(id.clone())
        .await
        .map_err(|e| format!("{org}: collection {id}: {e:?}"))?
    else {
        return Ok(None);
    };
    let nodes: Vec<NodeRef> = c.items.iter().map(|i| i.node.clone()).collect();
    // Positional and total: one answer per node, in order. A failed
    // batch only costs the titles.
    let resolved = links.resolve_nodes(nodes.clone()).await.unwrap_or_default();
    let published = |i: usize| resolved.get(i).map_or("", |r| r.title.as_str());
    let songs = nodes.iter().enumerate().map(|(i, node)| {
        let links = &links;
        let title = item_title(node, published(i));
        async move {
            SongRow {
                token: node.to_token(),
                title,
                projects: song_projects(&links_of(links, node.clone()).await, node),
            }
        }
    });
    let songs = futures_util::future::join_all(songs).await;
    let me = NodeRef::new(NodeKind::Collection, c.id.clone());
    let held_by = holders(&links_of(&links, me.clone()).await, &me);
    Ok(Some(SetlistDetail {
        title: c.title,
        kind: c.kind.as_str().to_owned(),
        held_by,
        songs,
    }))
}

/// The setlists a project holds: its `Down` links to collections, each
/// read back. A link whose collection is gone is left out.
async fn fetch_project_setlists(org: String, dir: String) -> Vec<SetlistRow> {
    let Ok(links) = crate::vox_clients::establish_for::<LinksServiceClient>(&org).await else {
        return Vec::new();
    };
    let project = NodeRef::new(NodeKind::Project, dir.clone());
    let ids: Vec<String> = links_of(&links, project.clone())
        .await
        .into_iter()
        .filter(|l| {
            l.source == project
                && l.relation == Relation::Down
                && l.target.kind == NodeKind::Collection
                && l.target.is_local()
        })
        .map(|l| l.target.id)
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(collections) = crate::vox_clients::establish_for::<CollectionServiceClient>(&org).await
    else {
        return Vec::new();
    };
    let found = futures_util::future::join_all(ids.into_iter().map(|id| {
        let collections = &collections;
        async move { collections.get(id).await.ok().flatten() }
    }))
    .await;
    let mut rows: Vec<SetlistRow> = found
        .into_iter()
        .flatten()
        .map(|c| SetlistRow {
            org: org.clone(),
            songs: c.items.len(),
            kind: c.kind.as_str().to_owned(),
            id: c.id,
            title: c.title,
            held_by: vec![dir.clone()],
        })
        .collect();
    rows.sort_by_key(|r| fold(&r.title));
    rows
}

// ── The list ────────────────────────────────────────────────────────

#[component]
pub fn SetlistsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let _projects = crate::stores::use_project_list();
    let project_store = crate::stores::use_project_store();

    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));
    let mut setlists = use_resource(move || fetch_setlists(slugs()));

    let index = use_memo(move || {
        let rows = project_store.list();
        project_index(rows.iter().map(|r| (r.slug.as_str(), &r.project)))
    });
    let many_orgs = slugs.read().len() > 1;

    let body = match &*setlists.read() {
        None => rsx! { crate::states::LoadingState { rows: 3 } },
        Some(Err(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load setlists",
                message: e.clone(),
                on_retry: move |()| setlists.restart(),
            }
        },
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            crate::states::EmptyState {
                title: "No setlists yet",
                hint: "Setlists made in an app that keeps them in Task show up here.".to_string(),
            }
        },
        Some(Ok(rows)) => {
            let groups = group_by_project(rows, &index.read());
            let orgs = org_list.read().clone();
            rsx! {
                for group in groups {
                    SetlistGroupView {
                        key: "{group_key(&group)}",
                        group,
                        orgs: orgs.clone(),
                        many_orgs,
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "mx-auto flex w-full max-w-4xl flex-col gap-8 p-4 pb-14 sm:p-6 md:pb-6 lg:p-10",
            header { class: "flex flex-col gap-1",
                Heading { level: HeadingLevel::H1, class: "text-3xl tracking-tight", "Setlists" }
                Text { variant: TextVariant::Muted,
                    "Every setlist in your organizations, by the project it belongs to."
                }
            }
            {body}
        }
    }
}

fn group_key(g: &SetlistGroup) -> String {
    g.project
        .as_ref()
        .map_or_else(|| "none".to_owned(), |p| format!("{}/{}", p.org, p.title))
}

#[component]
fn SetlistGroupView(group: SetlistGroup, orgs: Vec<OrgMeta>, many_orgs: bool) -> Element {
    let heading = match &group.project {
        Some(p) => {
            let org = org_name(&orgs, &p.org);
            rsx! {
                div { class: "flex flex-wrap items-baseline gap-x-2 gap-y-1",
                    if let Some(id) = p.id {
                        Link {
                            to: Route::ProjectDetailRoute { id: id.to_string() },
                            class: "text-lg font-semibold text-foreground hover:underline",
                            "{p.title}"
                        }
                    } else {
                        span { class: "text-lg font-semibold text-foreground", "{p.title}" }
                    }
                    if let Some(form) = p.form {
                        span { class: "text-[11px] font-semibold uppercase tracking-[0.18em] text-primary", "{form}" }
                    }
                    if many_orgs {
                        span { class: "text-sm text-muted-foreground", "{org}" }
                    }
                }
            }
        }
        None => rsx! {
            span { class: "text-lg font-semibold text-muted-foreground", "Not in a project" }
        },
    };
    rsx! {
        section { class: "flex flex-col gap-2",
            {heading}
            div { class: "flex flex-col divide-y divide-border/60 overflow-hidden rounded-xl border border-border/70 bg-card",
                for row in group.setlists.iter().cloned() {
                    SetlistRowView {
                        key: "{row.org}/{row.id}",
                        org_label: if many_orgs { org_name(&orgs, &row.org) } else { String::new() },
                        row,
                    }
                }
            }
        }
    }
}

#[component]
fn SetlistRowView(row: SetlistRow, org_label: String) -> Element {
    let songs = if row.songs == 1 {
        "1 song".to_owned()
    } else {
        format!("{} songs", row.songs)
    };
    rsx! {
        Link {
            to: Route::SetlistDetailRoute { org: row.org.clone(), id: row.id.clone() },
            class: "flex items-center gap-3 px-4 py-3 hover:bg-accent/20 transition-colors",
            span { class: "flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground",
                Music { size: 16 }
            }
            span { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-sm font-medium text-foreground", "{row.title}" }
                span { class: "truncate text-xs text-muted-foreground",
                    "{row.kind}"
                    if !org_label.is_empty() { " · {org_label}" }
                }
            }
            span { class: "shrink-0 text-xs tabular-nums text-muted-foreground", "{songs}" }
            span { class: "shrink-0 text-muted-foreground", ChevronRight { size: 14 } }
        }
    }
}

// ── One setlist ─────────────────────────────────────────────────────

#[component]
pub fn SetlistDetailView(org: String, id: String) -> Element {
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let _projects = crate::stores::use_project_list();
    let project_store = crate::stores::use_project_store();
    let key = use_memo({
        let (org, id) = (org.clone(), id.clone());
        move || (org.clone(), id.clone())
    });
    let mut detail = use_resource(move || {
        let (org, id) = key();
        fetch_setlist(org, id)
    });
    let index = use_memo(move || {
        let rows = project_store.list();
        project_index(rows.iter().map(|r| (r.slug.as_str(), &r.project)))
    });
    let org_label = org_name(&org_list.read(), &org);

    let body = match &*detail.read() {
        None => rsx! { crate::states::LoadingState { rows: 4 } },
        Some(Err(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load this setlist",
                message: e.clone(),
                on_retry: move |()| detail.restart(),
            }
        },
        Some(Ok(None)) => rsx! {
            crate::pages::missing::Missing {
                title: "No such setlist",
                detail: "`{org}` holds no collection `{id}`.",
            }
        },
        Some(Ok(Some(d))) => {
            let index = index.read();
            let holders: Vec<ProjectRef> = d
                .held_by
                .iter()
                .map(|dir| project_at(&index, &org, dir))
                .collect();
            let songs: Vec<(usize, SongRow, Vec<ProjectRef>)> = d
                .songs
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let projects = s
                        .projects
                        .iter()
                        .map(|dir| project_at(&index, &org, dir))
                        .collect();
                    (i + 1, s.clone(), projects)
                })
                .collect();
            rsx! {
                header { class: "flex flex-col gap-1.5",
                    div { class: "text-[11px] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        "{d.kind} · {org_label}"
                    }
                    Heading { level: HeadingLevel::H1, class: "text-3xl tracking-tight", "{d.title}" }
                    div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground",
                        span { class: "tabular-nums", "{d.songs.len()} songs" }
                        for p in holders {
                            span { "·" }
                            ProjectChip { project: p }
                        }
                    }
                }
                if songs.is_empty() {
                    crate::states::EmptyState { title: "This setlist is empty" }
                } else {
                    ol { class: "flex flex-col divide-y divide-border/60 overflow-hidden rounded-xl border border-border/70 bg-card",
                        for (n, song, projects) in songs {
                            li { key: "{n}-{song.token}", class: "flex items-center gap-3 px-4 py-2.5",
                                span { class: "w-6 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground",
                                    {format!("{n:02}")}
                                }
                                span { class: "min-w-0 flex-1 truncate text-sm text-foreground", title: "{song.token}",
                                    "{song.title}"
                                }
                                for p in projects {
                                    ProjectChip { project: p }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 pb-14 sm:p-6 md:pb-6 lg:p-10",
            Link {
                to: Route::SetlistsRoute {},
                class: "text-sm text-muted-foreground hover:text-foreground",
                "← All setlists"
            }
            {body}
        }
    }
}

/// A project named on a setlist or a song: a link when Task lists it.
#[component]
fn ProjectChip(project: ProjectRef) -> Element {
    let class =
        "shrink-0 rounded-full border border-border px-2 py-0.5 text-[11px] text-muted-foreground";
    match project.id {
        Some(id) => rsx! {
            Link {
                to: Route::ProjectDetailRoute { id: id.to_string() },
                class: "{class} hover:text-foreground hover:border-foreground/40",
                "{project.title}"
            }
        },
        None => rsx! { span { class, "{project.title}" } },
    }
}

// ── On a project's page ─────────────────────────────────────────────

/// The setlists a project holds, for its page: a series lists every
/// week's, an album its running order. Renders nothing for a project
/// that holds none, so a project with no setlists carries no empty box.
#[component]
pub fn ProjectSetlists(project: ProjectInfo, slug: String) -> Element {
    let key = use_memo(use_reactive!(|(project, slug)| (
        slug.clone(),
        project_dir(&project.path)
    )));
    let rows = use_resource(move || {
        let (org, dir) = key();
        fetch_project_setlists(org, dir)
    });
    let Some(rows) = rows.read().clone() else {
        return rsx! {};
    };
    if rows.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-col gap-2", "data-testid": "project-setlists",
            Heading { level: HeadingLevel::H2, "Setlists" }
            div { class: "flex flex-col divide-y divide-border/60 overflow-hidden rounded-xl border border-border/70 bg-card",
                for row in rows {
                    SetlistRowView { key: "{row.id}", org_label: String::new(), row }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use links_proto::Confidence;

    fn link(source: NodeRef, target: NodeRef, relation: Relation) -> TypedLink {
        TypedLink::new(source, target, relation, Confidence::Certain)
    }

    fn row(org: &str, id: &str, title: &str, held_by: &[&str]) -> SetlistRow {
        SetlistRow {
            org: org.into(),
            id: id.into(),
            title: title.into(),
            kind: "setlist".into(),
            songs: 0,
            held_by: held_by.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn project(org: &str, dir: &str, title: &str) -> ((String, String), ProjectRef) {
        (
            (org.into(), dir.into()),
            ProjectRef {
                org: org.into(),
                id: Some(Uuid::nil()),
                title: title.into(),
                form: None,
            },
        )
    }

    #[test]
    fn only_setlists_and_songlists_are_listed() {
        assert!(is_setlist_kind("setlist"));
        assert!(is_setlist_kind("songlist"));
        assert!(!is_setlist_kind("library"));
        assert!(!is_setlist_kind(""));
    }

    #[test]
    fn a_project_goes_by_the_directory_its_page_is_in() {
        assert_eq!(
            project_dir("crescendum-album/project.md"),
            "crescendum-album"
        );
        assert_eq!(
            project_dir("example-album/track-two/project.md"),
            "example-album/track-two"
        );
        assert_eq!(project_dir("Projects/foo.md"), "Projects/foo");
        assert_eq!(project_dir("myproject.md"), "myproject");
    }

    #[test]
    fn a_slug_reads_aloud() {
        assert_eq!(humanise_slug("cinematic-intro"), "Cinematic Intro");
        assert_eq!(humanise_slug("know_me"), "Know Me");
        assert_eq!(humanise_slug("intro"), "Intro");
        assert_eq!(humanise_slug("--"), "");
    }

    #[test]
    fn a_song_is_called_by_its_published_title_else_its_slug() {
        let song = NodeRef::song("it-knows-my-name");
        assert_eq!(
            item_title(&song, "It Knows My Name (Live)"),
            "It Knows My Name (Live)"
        );
        assert_eq!(item_title(&song, "  "), "It Knows My Name");
    }

    #[test]
    fn a_collection_is_held_by_the_projects_that_link_down_to_it() {
        let set = NodeRef::new(NodeKind::Collection, "c1");
        let album = NodeRef::new(NodeKind::Project, "crescendum-album");
        let links = vec![
            link(album.clone(), set.clone(), Relation::Down),
            link(album, set.clone(), Relation::Down),
            // Up is not belonging; another org's project is not ours.
            link(
                NodeRef::new(NodeKind::Project, "x"),
                set.clone(),
                Relation::Up,
            ),
            link(
                NodeRef::new(NodeKind::Project, "y").in_domain("other.test"),
                set.clone(),
                Relation::Down,
            ),
        ];
        assert_eq!(holders(&links, &set), vec!["crescendum-album".to_owned()]);
    }

    #[test]
    fn a_song_belongs_to_a_project_linked_either_way() {
        let song = NodeRef::song("unbreakable");
        let links = vec![
            link(
                NodeRef::new(NodeKind::Project, "unbreakable"),
                song.clone(),
                Relation::Down,
            ),
            link(
                song.clone(),
                NodeRef::new(NodeKind::Project, "crescendum-album"),
                Relation::Related,
            ),
            link(
                song.clone(),
                NodeRef::new(NodeKind::Project, "z"),
                Relation::Cites,
            ),
            link(song.clone(), NodeRef::song("other"), Relation::Related),
        ];
        assert_eq!(
            song_projects(&links, &song),
            vec!["crescendum-album".to_owned(), "unbreakable".to_owned()]
        );
    }

    #[test]
    fn setlists_group_by_project_with_the_loose_ones_last() {
        let index: ProjectIndex = [
            project("fta", "crescendum-album", "Crescendum (album)"),
            project("fta", "jhm-sundays", "JHM Sundays"),
        ]
        .into_iter()
        .collect();
        let rows = vec![
            row("fta", "w2", "2026-10-11", &["jhm-sundays"]),
            row("fta", "w1", "2026-10-04", &["jhm-sundays"]),
            row("fta", "c", "Crescendum", &["crescendum-album"]),
            row("tbm", "png", "PNG Worship Collective", &[]),
            row("fta", "jt", "JT's Senior Recital", &[]),
        ];
        let groups = group_by_project(&rows, &index);
        let shape: Vec<(Option<String>, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.project.as_ref().map(|p| p.title.clone()),
                    g.setlists.iter().map(|s| s.title.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("Crescendum (album)".into()), vec!["Crescendum"]),
                (Some("JHM Sundays".into()), vec!["2026-10-04", "2026-10-11"]),
                (None, vec!["JT's Senior Recital", "PNG Worship Collective"]),
            ]
        );
    }

    #[test]
    fn a_setlist_two_projects_hold_shows_under_both() {
        let index: ProjectIndex = [project("o", "a", "A"), project("o", "b", "B")]
            .into_iter()
            .collect();
        let groups = group_by_project(&[row("o", "s", "Set", &["a", "b"])], &index);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.setlists.len() == 1));
    }

    #[test]
    fn a_link_to_an_unlisted_project_still_heads_a_group() {
        let groups = group_by_project(&[row("o", "s", "Set", &["old-tour"])], &ProjectIndex::new());
        let p = groups[0].project.as_ref().unwrap();
        assert_eq!((p.title.as_str(), p.id), ("Old Tour", None));
    }
}
