//! Deliverable playback — the master, right on the page.
//!
//! A project's main deliverable is the thing you came for: an album's
//! audio, a documentary's cut. The model declares WHAT is owed
//! (`ProjectInfo::deliverables`, expanded to items by
//! `deliverable_items`); the *bytes* live by convention in the org's
//! media tree at `deliverables/<project-dir>/<item title>.<ext>`,
//! served by `GET /org/{slug}/media/…` (Range-capable, so browser
//! players stream and seek) behind a short-lived signed grant — the
//! same route and grant the stem player and project covers use.
//!
//! Convention rather than a stored binding, deliberately: the model's
//! own docs put binding on the Files layer's future; until that lands,
//! "the file named like the item, in the deliverables folder" is a
//! binding a human can see in a file manager and repair with a rename.
//!
//! An item whose file is missing is *outstanding* — the declared-and-
//! unbound state the spec names — and renders as a quiet chip, never a
//! broken player: the `<audio>/<video>` error event swaps the element
//! out.

use dioxus::prelude::*;
use project_proto::{DeliverableItem, Medium, ProjectInfo};

/// The media directory for a project's non-audio deliverables —
/// `Projects/example-album.md` → `deliverables/example-album`. Audio
/// takes the other road: it lives as *songs* (see [`song_slug`]) so the
/// platform's own player streams it.
pub(super) fn media_dir(p: &ProjectInfo) -> String {
    let stem = std::path::Path::new(&p.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    format!("deliverables/{stem}")
}

/// An audio deliverable item's song slug — the colocated-song folder
/// the global player streams (`/org/{org}/media/songs/{slug}/…`).
/// Slugified from the item's title the same way the seed names its
/// song folders: lowercase, every non-alphanumeric run one dash.
pub(super) fn song_slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Fire the global player with a queue. The player owns its copy of
/// the queue from here — navigation, skip and the dock transport are
/// all its business (`task-player-ui`), exactly why this beats a bare
/// `<audio>` element. Takes the context handle (consume it at render
/// time; this runs in event handlers, where hooks are off-limits).
pub(super) fn play_queue(
    np: crate::chrome::NowPlaying,
    org: &str,
    title: &str,
    songs: Vec<String>,
    start: usize,
) {
    let mut np = np.0;
    let generation = np.peek().generation + 1;
    np.set(crate::chrome::NowPlayingRequest {
        generation,
        org: org.to_owned(),
        title: title.to_owned(),
        songs,
        start,
        toggle: false,
    });
}

/// File extensions worth offering a browser `<source>` chain for, per
/// medium. The seed ships WAV; real projects will drop OGG/MP3/MP4 in
/// the same folder and the first source that loads wins.
fn extensions(medium: Medium) -> &'static [&'static str] {
    match medium {
        Medium::Audio => &["wav", "ogg", "mp3", "flac"],
        Medium::Video => &["mp4", "webm"],
        Medium::Image => &["png", "jpg", "webp"],
        Medium::Document => &["pdf"],
    }
}

/// Signed URLs for one item, one per candidate extension. `None` when
/// no grant could be minted (offline / unauthorised) — the caller
/// renders the outstanding state rather than a 401 in a player.
async fn item_sources(slug: &str, dir: &str, item: &DeliverableItem) -> Option<Vec<String>> {
    let suffix = task_ui_core::media_grant::suffix_for_prefix(slug, dir).await;
    if suffix.is_empty() {
        return None;
    }
    // Always the server's base, never a relative URL: a relative path
    // resolves against whatever origin served the APP — in a dev split
    // (dx on one port, the server on another) that's the static server,
    // which silently has no `/org/…`. `http_base` derives the right
    // answer on both targets (baked dev URL, same-origin in prod).
    let base = task_ui_core::orgs::http_base();
    Some(
        extensions(item.medium)
            .iter()
            .map(|ext| format!("{base}/org/{slug}/media/{dir}/{}.{ext}{suffix}", item.title))
            .collect(),
    )
}

/// The masters — every whole-project deliverable item, playable in
/// place. Per-part items ride the parts list instead (same folder,
/// titled by the part).
#[component]
pub(super) fn MasterDeliverables(project: ProjectInfo, slug: String) -> Element {
    let dir = media_dir(&project);
    let pid = project.id;
    let slug_for_fetch = slug.clone();
    // The expansion is the server's (`deliverable_items` — derived, so
    // it stays in step with the parts); whole-project items only here.
    let items = use_resource(use_reactive!(|(pid,)| {
        let slug = slug_for_fetch.clone();
        async move {
            let client: project_proto::ProjectServiceClient =
                task_ui_core::vox_clients::establish_for(&slug).await.ok()?;
            let all = client.deliverable_items(pid).await.ok()?;
            Some(
                all.into_iter()
                    .filter(|i| i.part.is_none())
                    .collect::<Vec<_>>(),
            )
        }
    }));

    let list: Vec<DeliverableItem> = items
        .read_unchecked()
        .as_ref()
        .cloned()
        .flatten()
        .unwrap_or_default();
    if list.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-col gap-3",
            for item in list.iter().cloned() {
                MasterItem {
                    key: "{item.deliverable}-{item.title}",
                    item,
                    slug: slug.clone(),
                    dir: dir.clone(),
                    project_title: project.title.clone(),
                }
            }
        }
    }
}

/// One whole-project master. Audio routes through the GLOBAL player —
/// the dock transport, the queue, playback that survives navigation.
/// Video routes through the REVIEW surface — the platform's own player
/// (frame-anchored comments, versions, the full screen behind the
/// expand button), mounted on the deliverable inside the File Root
/// named after the project. Both beat a bare element for the same
/// reason: the platform already has the better UI.
#[component]
fn MasterItem(item: DeliverableItem, slug: String, dir: String, project_title: String) -> Element {
    let medium = item.medium;
    let title = item.title.clone();
    let np = use_context::<crate::chrome::NowPlaying>();

    // The item's home in the project's File Root: the file in
    // `Deliverables/` named like the item, whatever its extension.
    // Video mounts the review mini player on it; audio gets a Review
    // door beside Play. `None` (root not adopted / not delivered yet)
    // falls through to the media-tree convention, then Outstanding.
    let slug_for_review = slug.clone();
    let title_for_review = project_title.clone();
    let item_for_review = item.clone();
    let review_target = use_resource(move || {
        let slug = slug_for_review.clone();
        let project = title_for_review.clone();
        let item = item_for_review.clone();
        async move {
            if !matches!(item.medium, Medium::Video | Medium::Audio) {
                return None;
            }
            files_ui::review::locate_titled(&slug, &project, &item.title).await
        }
    });
    let target: Option<(uuid::Uuid, String)> =
        review_target.read_unchecked().as_ref().cloned().flatten();
    // The unified media session: opening a deliverable hands it to the
    // shell's host (zoomed review now, dock strip when you leave).
    let session = use_context::<crate::media_session::MediaSession>();

    // Non-audio media resolves by the deliverables-folder convention.
    let item_for_srcs = item.clone();
    let slug_for_srcs = slug.clone();
    let dir_for_srcs = dir.clone();
    let sources = use_resource(move || {
        let slug = slug_for_srcs.clone();
        let dir = dir_for_srcs.clone();
        let item = item_for_srcs.clone();
        async move {
            if item.medium == Medium::Audio {
                return None;
            }
            item_sources(&slug, &dir, &item).await
        }
    });
    let srcs: Option<Vec<String>> = sources.read_unchecked().as_ref().cloned().flatten();
    // The error path: every candidate source failed to load — the item
    // is declared and not (yet) delivered.
    let mut failed = use_signal(|| false);

    let openable = target.is_some();
    // Every deliverable is a CARD with the same anatomy — icon, title,
    // medium chip, media below — sized to its content (the video is
    // 16:9; a card stretched to the page just frames dead space). The
    // whole header row IS the affordance: click it to open the review;
    // on an audio item the icon trades itself for a play button on
    // hover, so listening stays one click without a button rack.
    rsx! {
        div { class: "w-full max-w-[707px] overflow-hidden rounded-xl border border-border/70 bg-card/50",
            div {
                class: if openable {
                    "group/head flex cursor-pointer items-center gap-2.5 px-4 py-2.5 transition-colors hover:bg-accent/20"
                } else {
                    "group/head flex items-center gap-2.5 px-4 py-2.5"
                },
                onclick: {
                    let slug = slug.clone();
                    let title = title.clone();
                    let target = target.clone();
                    move |_| {
                        if let Some((root_id, path)) = target.clone() {
                            session.open(crate::media_session::ReviewMedia {
                                org: slug.clone(),
                                root_id,
                                path,
                                title: title.clone(),
                            });
                        }
                    }
                },
                if medium == Medium::Audio {
                    {
                        let org = slug.clone();
                        let queue_title = title.clone();
                        let song = song_slug(&title);
                        rsx! {
                            button {
                                r#type: "button",
                                title: "Play",
                                class: "flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary transition-colors hover:bg-primary hover:text-primary-foreground",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    play_queue(np, &org, &queue_title, vec![song.clone()], 0);
                                },
                                span { class: "group-hover/head:hidden", {medium_icon(medium)} }
                                span { class: "hidden group-hover/head:block",
                                    architect_ui::lucide_dioxus::Play { size: 14 }
                                }
                            }
                        }
                    }
                } else {
                    span { class: "flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary",
                        {medium_icon(medium)}
                    }
                }
                span { class: "min-w-0 truncate text-sm font-semibold", "{title}" }
                span { class: "shrink-0 rounded-full border border-border/60 px-1.5 py-px text-[10px] uppercase tracking-widest text-muted-foreground",
                    {medium_label(medium)}
                }
                // The quiet cue that the row opens somewhere.
                if openable {
                    span { class: "ml-auto flex items-center gap-1 text-[11px] text-muted-foreground opacity-0 transition-opacity group-hover/head:opacity-100",
                        "Review"
                        architect_ui::lucide_dioxus::ChevronRight { size: 12 }
                    }
                }
            }
            if medium == Medium::Audio {
            } else if let Some((root_id, path)) = target.clone() {
                // The review platform's own player: stage, marker
                // timeline, transport, and the expand button that opens
                // the full review screen (comments and all) over the
                // viewport.
                div { class: "border-t border-border/40 p-3",
                    // 683px = the 16:9 width of the mini stage's
                    // `max-h-96` cap — any wider and the stage gains
                    // letterbox side-bars inside its own rounded box.
                    div { class: "max-w-[683px]",
                        files_ui::review::MiniPlayer {
                            org: slug.clone(),
                            root_id,
                            path: path.clone(),
                            // Expand goes through the unified session,
                            // not a private overlay — same screen the
                            // header row and the dock strip open.
                            on_expand: {
                                let slug = slug.clone();
                                let title = title.clone();
                                move |()| {
                                    session.open(crate::media_session::ReviewMedia {
                                        org: slug.clone(),
                                        root_id,
                                        path: path.clone(),
                                        title: title.clone(),
                                    });
                                }
                            },
                        }
                    }
                }
            } else if medium == Medium::Video && review_target.read_unchecked().is_none() {
                // Still asking the Files service — don't flash the
                // fallback while the real answer is in flight.
                div { class: "border-t border-border/40 px-4 py-3",
                    span { class: "text-xs text-muted-foreground", "…" }
                }
            } else {
                div { class: "border-t border-border/40 px-4 py-3",
                    match (&srcs, failed()) {
                        (_, true) => rsx! { Outstanding {} },
                        (None, _) => rsx! {
                            span { class: "text-xs text-muted-foreground", "…" }
                        },
                        (Some(srcs), _) => rsx! {
                            MediaElement {
                                medium,
                                sources: srcs.clone(),
                                on_failed: move |()| failed.set(true),
                            }
                        },
                    }
                }
            }
        }
    }
}

/// The medium's icon — the card's leading glyph.
fn medium_icon(m: Medium) -> Element {
    use architect_ui::lucide_dioxus::{FileText, Film, Image as ImageIcon, Music};
    match m {
        Medium::Audio => rsx! { Music { size: 14 } },
        Medium::Video => rsx! { Film { size: 14 } },
        Medium::Image => rsx! { ImageIcon { size: 14 } },
        Medium::Document => rsx! { FileText { size: 14 } },
    }
}

/// The player for a medium, fed a `<source>` chain so the first
/// extension that exists wins. `on_failed` fires only when the LAST
/// source errors — before that, source errors are the chain working.
#[component]
pub(super) fn MediaElement(
    medium: Medium,
    sources: Vec<String>,
    on_failed: EventHandler<()>,
) -> Element {
    let last = sources.len().saturating_sub(1);
    match medium {
        Medium::Audio => rsx! {
            audio { controls: true, preload: "metadata", class: "w-full max-w-xl",
                for (i, src) in sources.iter().enumerate() {
                    source {
                        key: "{src}",
                        src: "{src}",
                        onerror: move |_| {
                            if i == last {
                                on_failed.call(());
                            }
                        },
                    }
                }
            }
        },
        Medium::Video => rsx! {
            // `preload=metadata` pulls the first frame — the thumbnail —
            // without downloading the cut.
            video {
                controls: true,
                preload: "metadata",
                class: "aspect-video w-full max-w-2xl rounded-xl border border-border bg-black",
                for (i, src) in sources.iter().enumerate() {
                    source {
                        key: "{src}",
                        src: "{src}",
                        onerror: move |_| {
                            if i == last {
                                on_failed.call(());
                            }
                        },
                    }
                }
            }
        },
        Medium::Image => rsx! {
            img {
                src: sources.first().cloned().unwrap_or_default(),
                class: "max-w-xl rounded-xl border border-border",
                onerror: move |_| on_failed.call(()),
            }
        },
        Medium::Document => rsx! {
            a {
                href: sources.first().cloned().unwrap_or_default(),
                target: "_blank",
                class: "text-sm text-primary hover:underline",
                "Open document"
            }
        },
    }
}

/// Declared, nothing delivered yet — the honest state, quietly.
#[component]
pub(super) fn Outstanding() -> Element {
    rsx! {
        span { class: "w-fit rounded-full border border-dashed border-border px-2 py-0.5 text-[11px] text-muted-foreground",
            "outstanding — nothing delivered yet"
        }
    }
}

fn medium_label(m: Medium) -> &'static str {
    match m {
        Medium::Audio => "audio",
        Medium::Video => "video",
        Medium::Image => "image",
        Medium::Document => "document",
    }
}
