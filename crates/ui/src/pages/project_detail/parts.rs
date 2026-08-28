//! Parts — the tracklist.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use project_proto::{Medium, ProjectInfo, Scope};
use uuid::Uuid;

use crate::stores;

use super::deliverable::{play_queue, song_slug};

/// One row of the tracklist. On a playable project the whole row is a
/// play control: the number swaps to ▶ on hover, and a click hands the
/// album to the global player starting at this track.
#[component]
fn PartRow(
    index: usize,
    part: project_proto::Part,
    playable: bool,
    on_play: EventHandler<usize>,
) -> Element {
    let row_class = if playable {
        "group flex items-center gap-3 px-4 py-2.5 cursor-pointer hover:bg-accent/20 transition-colors"
    } else {
        "flex items-center gap-3 px-4 py-2.5"
    };
    rsx! {
        div {
            class: "{row_class}",
            onclick: move |_| {
                if playable {
                    on_play.call(index);
                }
            },
            if playable {
                span { class: "relative w-6 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground",
                    span { class: "group-hover:invisible", {format!("{:02}", index + 1)} }
                    span { class: "invisible absolute inset-y-0 right-0 text-primary group-hover:visible", "▶" }
                }
            } else {
                span { class: "w-6 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground",
                    {format!("{:02}", index + 1)}
                }
            }
            span { class: "min-w-0 flex-1 truncate text-sm text-foreground", "{part.name}" }
            if part.references.is_some() {
                span { class: "shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] uppercase tracking-wide text-muted-foreground",
                    title: "A setlist entry — this references a piece owned elsewhere",
                    "ref"
                }
            }
            for (ci, c) in part.components.iter().enumerate() {
                span {
                    key: "{ci}",
                    class: "shrink-0 rounded-full bg-muted/60 px-2 py-0.5 text-[10px] text-muted-foreground",
                    "{component_label(c.kind)} · {c.name}"
                }
            }
        }
    }
}

/// What one component is called in a row's chip.
fn component_label(k: project_proto::ComponentKind) -> &'static str {
    match k {
        project_proto::ComponentKind::Chart => "chart",
        project_proto::ComponentKind::Session => "session",
        project_proto::ComponentKind::Score => "score",
        project_proto::ComponentKind::Lyrics => "lyrics",
    }
}

/// The project's parts as a numbered list — an album's tracklist, a
/// documentary's scenes. The numbering is information, not decoration:
/// parts are ordered units of the work (`project.part.unit`), and the
/// order shown is the order the page's frontmatter declares.
///
/// Adding a part is one input away: the part rides `ProjectInfo`
/// itself, so the optimistic project store's `update` is the whole
/// write path — same semantics as the backend's `add_part`.
#[component]
pub(super) fn PartsSection(project: ProjectInfo, slug: String) -> Element {
    let project_muts = stores::use_project_mutations();
    let mut draft = use_signal(String::new);
    // A part's main deliverable: when the project declares a per-part
    // audio deliverable, every part IS a track — clicking one hands the
    // whole tracklist to the global player as a queue, starting there.
    // The player (task-player-ui) owns it from that moment: the dock
    // transport, skip within the album, playback that survives leaving
    // the page.
    let playable = project
        .deliverables
        .0
        .iter()
        .any(|d| d.medium == Medium::Audio && d.scope == Scope::PerPart);
    let np = use_context::<crate::chrome::NowPlaying>();
    let queue: Vec<String> = project.parts.0.iter().map(|x| song_slug(&x.name)).collect();
    let album_title = project.title.clone();

    let add = use_callback({
        let p = project.clone();
        let slug = slug.clone();
        move |()| {
            let name = draft.peek().trim().to_owned();
            if name.is_empty() || p.parts.0.iter().any(|x| x.name == name) {
                return;
            }
            let mut np = p.clone();
            np.parts.0.push(project_proto::Part {
                id: Uuid::new_v4(),
                name,
                references: None,
                components: Vec::new(),
            });
            project_muts.update(slug.clone(), np);
            draft.set(String::new());
        }
    });

    // A project WITHOUT parts gets no Parts scaffolding — no heading,
    // no explainer box. Just one quiet add-row at the bottom of the
    // overview: a single that will never have parts shouldn't page
    // around an empty concept, and the affordance is still one glance
    // away for the project that's about to become an album.
    if project.parts.0.is_empty() {
        return rsx! {
            div {
                class: "flex max-w-md items-center gap-2 pt-2 opacity-70 transition-opacity focus-within:opacity-100 hover:opacity-100",
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        add.call(());
                    }
                },
                Input {
                    value: draft,
                    size: InputSize::Small,
                    placeholder: "Add a part — a song, a scene, an episode…",
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| add.call(()),
                    "Add"
                }
            }
        };
    }

    rsx! {
        div { class: "flex flex-col gap-2",
            div { class: "flex items-baseline gap-2",
                Heading { level: HeadingLevel::H2, class: "text-lg font-semibold", "Parts" }
                span { class: "text-xs tabular-nums text-muted-foreground", "· {project.parts.0.len()}" }
            }
            div { class: "flex flex-col divide-y divide-border/60 rounded-xl border border-border bg-card/40",
                    for (i, part) in project.parts.0.iter().enumerate() {
                        PartRow {
                            key: "{part.id}",
                            index: i,
                            part: part.clone(),
                            playable,
                            on_play: {
                                let queue = queue.clone();
                                let title = album_title.clone();
                                let org = slug.clone();
                                move |idx: usize| {
                                    play_queue(np, &org, &title, queue.clone(), idx);
                                }
                            },
                        }
                    }
            }
            div {
                class: "flex max-w-md items-center gap-2",
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        add.call(());
                    }
                },
                Input {
                    value: draft,
                    size: InputSize::Small,
                    placeholder: "Add a part — a song, a scene, an episode…",
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| add.call(()),
                    "Add"
                }
            }
        }
    }
}
