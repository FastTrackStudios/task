//! Watch — a video, its transcript, and timestamped notes on it.
//!
//! Embeds the video via the YouTube IFrame API (`enablejsapi=1`) and
//! drives it over the postMessage channel: clicking a moment seeks the
//! player, and "add note at current time" reads `getCurrentTime` back
//! and writes a `TypedLink` from `<node>#t:<secs>`. The moments come
//! from `LinksService.links_for`, so this works for any `video:` /
//! `sermon:` / `song:` node.
//!
//! Mounted by `task-plugin-studio`.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use links_proto::{
    Anchor, Confidence, NodeKind, NodeRef, Relation, TypedLink, Visibility, format_timecode,
};
use task_ui_core::feeds;
use task_ui_core::orgs::{OrgMeta, OrgSelection, selected_slugs};

/// This app's id in Task's catalog, and the first segment of every
/// link it writes to itself.
pub const APP_ID: &str = "fasttrackstudio";

/// Every typed link touching `node_token` (a `kind:id` NodeRef token,
/// e.g. `sermon:god-restores-broken-people`) — the timestamped notes
/// this view renders.
///
/// Declared here rather than reached for through the shell: the links
/// service is a service this app calls. The vault page reads it too,
/// for its own reasons, and declares its own.
pub async fn fetch_links_for(
    slug: &str,
    node_token: &str,
) -> Result<Vec<links_proto::TypedLink>, String> {
    let node = links_proto::NodeRef::parse(node_token)
        .ok_or_else(|| format!("bad node token: {node_token}"))?;
    let client =
        task_ui_core::vox_clients::establish_for::<links_proto::LinksServiceClient>(slug).await?;
    client
        .links_for(node)
        .await
        .map_err(|e| format!("{slug}: links_for: {e:?}"))
}

/// view's synced transcript. Empty vec on a missing sidecar.
pub async fn fetch_transcript(
    slug: &str,
    rel_path: &str,
) -> Result<Vec<resources_proto::TranscriptSegment>, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<resources_proto::ResourcesServiceClient>(slug)
            .await?;
    // A missing / unreadable transcript just means no cues — never fatal.
    Ok(client
        .transcript(rel_path.to_owned())
        .await
        .map(|doc| doc.segments)
        .unwrap_or_default())
}

/// Save a watched video to the library as a `type: video` vault note
/// (`Videos/<id>.md`) so `[[id]]` resolves and it shows in a Videos base.
/// `CreateOnly`, so re-saving an existing video is a no-op (the title
/// stays whatever you renamed it to).
pub async fn save_video_note(
    slug: &str,
    video_id: &str,
    url: &str,
    title: &str,
) -> Result<(), String> {
    let title = if title.trim().is_empty() {
        video_id
    } else {
        title
    };
    let md = format!(
        "---\ntitle: {title}\ntype: video\nkind: video\nvideo_id: {video_id}\nurl: {url}\ntags: [video]\n---\n\n# {title}\n\nTimestamped notes are typed links on `video:{video_id}`. Watch + annotate at `/watch?v={video_id}&node=video:{video_id}`.\n"
    );
    let client =
        task_ui_core::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    client
        .put_file(
            "default".to_owned(),
            format!("Videos/{video_id}.md"),
            md.into_bytes(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("{slug}: save video: {e:?}"))
}

feeds! {
    links_proto::LinksServiceClient {
        /// Persist one typed link (the watch view's "add note at current time").
        create_link(link: links_proto::TypedLink) -> links_proto::TypedLink
            = create(link) as "create link";

    }
}

// ─────────────────────────────────────────────────────────────────────
// The screen
// ─────────────────────────────────────────────────────────────────────

const EMBED_ID: &str = "watch-embed";

/// The transcript sidecar path (resources-relative) for a media node —
/// `sermon:slug` → `sermons/slug.transcript.json`. `None` for kinds that
/// don't carry a transcript.
fn transcript_rel_path(node_token: &str) -> Option<String> {
    let node = NodeRef::parse(node_token)?;
    let kind = node.kind.as_str();
    matches!(kind, "sermon" | "video" | "song")
        .then(|| format!("{kind}s/{}.transcript.json", node.id))
}

/// Parse the optional "link to" field into a target node: a full
/// `kind:id` token, else an OSIS-looking verse (`John.3.16`), else a
/// topic slug. `None` when blank → the note targets the video itself.
fn parse_target(s: &str) -> Option<NodeRef> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(n) = NodeRef::parse(s) {
        return Some(n);
    }
    if s.contains('.') {
        return Some(NodeRef::verse(s));
    }
    Some(NodeRef::new(NodeKind::Topic, s))
}

/// YouTube video id out of a URL or a bare id.
fn youtube_id(input: &str) -> Option<String> {
    let s = input.trim();
    let rest = if let Some(i) = s.find("v=") {
        &s[i + 2..]
    } else if let Some(i) = s.find("youtu.be/") {
        &s[i + 9..]
    } else if let Some(i) = s.find("/embed/") {
        &s[i + 7..]
    } else {
        s
    };
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (id.len() >= 6).then_some(id)
}

/// Seek (and play) the embedded player via the IFrame postMessage API.
fn seek_embed(sec: u32) {
    let js = format!(
        "const f=document.getElementById('{EMBED_ID}');\
if(f&&f.contentWindow){{\
f.contentWindow.postMessage(JSON.stringify({{event:'listening',id:'{EMBED_ID}'}}),'*');\
f.contentWindow.postMessage(JSON.stringify({{event:'command',func:'seekTo',args:[{sec},true]}}),'*');\
f.contentWindow.postMessage(JSON.stringify({{event:'command',func:'playVideo',args:[]}}),'*');\
}}"
    );
    let _ = dioxus::document::eval(&js);
}

/// Install a one-time `message` listener that captures the player's
/// `currentTime` from YouTube `infoDelivery` events into a JS global, and
/// (re)send the `listening` handshake so those events start flowing.
fn install_time_listener() {
    let js = format!(
        "if(!window.__watchInit){{window.__watchInit=true;window.__watchTime=0;\
window.addEventListener('message',function(e){{if(typeof e.data!=='string')return;\
try{{var d=JSON.parse(e.data);if(d.event==='infoDelivery'&&d.info&&typeof d.info.currentTime==='number'){{window.__watchTime=d.info.currentTime;}}}}catch(_e){{}}}});}}\
function hs(){{var f=document.getElementById('{EMBED_ID}');if(f&&f.contentWindow){{f.contentWindow.postMessage(JSON.stringify({{event:'listening',id:'{EMBED_ID}'}}),'*');}}}}\
hs();setTimeout(hs,1000);setTimeout(hs,3000);"
    );
    let _ = dioxus::document::eval(&js);
}

/// Read the player's current time (whole seconds) back into Rust.
async fn current_time() -> u32 {
    let mut e = dioxus::document::eval("dioxus.send(Math.floor(window.__watchTime||0));");
    e.recv::<f64>().await.map(|t| t as u32).unwrap_or(0)
}

/// One timestamped note on the video.
#[derive(Clone, PartialEq)]
struct Moment {
    start: u32,
    label: String,
    note: String,
    target: Option<String>,
}

/// Project a link into a moment iff its *source* is this node at a time.
fn moment(link: &TypedLink, node: &NodeRef) -> Option<Moment> {
    if link.source.kind != node.kind || link.source.id != node.id {
        return None;
    }
    let (start, label) = match link.source.anchor_kind() {
        Anchor::Timestamp(s) => (s, format_timecode(s)),
        Anchor::Clip { start, end } => (
            start,
            format!("{}–{}", format_timecode(start), format_timecode(end)),
        ),
        _ => return None,
    };
    let target = (link.target.kind != node.kind || link.target.id != node.id)
        .then(|| link.target.to_token());
    Some(Moment {
        start,
        label,
        note: link.note.clone(),
        target,
    })
}

#[component]
pub fn WatchView(v: String, node: String) -> Element {
    let nav = use_navigator();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The node these notes hang on: explicit `node`, else `video:<v>`.
    let node_token = if !node.is_empty() {
        node.clone()
    } else if !v.is_empty() {
        format!("video:{v}")
    } else {
        String::new()
    };

    // No video → a paste-a-URL landing.
    if v.is_empty() {
        return rsx! { UrlPrompt {} };
    }

    let mut note_text = use_signal(String::new);
    let mut target_text = use_signal(String::new);
    let mut clip_in = use_signal(|| None::<u32>);
    let mut saved = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);
    let vid = v.clone();

    // Wire the player message channel once the embed is in the DOM.
    use_effect(move || {
        let _ = vid.clone();
        install_time_listener();
    });

    let nt = node_token.clone();
    let moments = use_resource(move || {
        let nt = nt.clone();
        async move {
            let _ = refresh();
            let slug = selected_slugs(&selection.read(), &org_list.read())
                .first()
                .cloned()?;
            let node = NodeRef::parse(&nt)?;
            let links = self::fetch_links_for(&slug, &nt).await.ok()?;
            let mut ms: Vec<Moment> = links.iter().filter_map(|l| moment(l, &node)).collect();
            ms.sort_by_key(|m| m.start);
            Some(ms)
        }
    });

    // Capture the current playhead as the clip start.
    let mark_in = use_callback(move |()| {
        spawn(async move {
            clip_in.set(Some(current_time().await));
        });
    });

    // Add a note at the current moment — a clip (`#t:in-out`) when an
    // in-point is marked, else a point (`#t:secs`). Targets the typed
    // node in "link to" if given (relation `alludes-to`), else the video
    // itself (`mentions`).
    // Synced transcript (sermons today; videos/songs when present).
    let tnt = node_token.clone();
    let transcript = use_resource(move || {
        let tnt = tnt.clone();
        async move {
            let slug = selected_slugs(&selection.read(), &org_list.read())
                .first()
                .cloned()?;
            let rel = transcript_rel_path(&tnt)?;
            self::fetch_transcript(&slug, &rel).await.ok()
        }
    });

    let add_token = node_token.clone();
    let add_note = use_callback(move |()| {
        let slug = selected_slugs(&selection.read(), &org_list.read())
            .first()
            .cloned();
        let token = add_token.clone();
        let text = note_text();
        let target_raw = target_text();
        let start = clip_in();
        if text.trim().is_empty() {
            return;
        }
        spawn(async move {
            let now = current_time().await;
            if let (Some(slug), Some(node)) = (slug, NodeRef::parse(&token)) {
                // Clip when an in-point is set (ordered); else a point.
                let source = match start {
                    Some(a) if now > a => node.clone().clip(a, now),
                    Some(a) => node.clone().clip(now, a),
                    None => node.clone().at(now),
                };
                let (target, relation) = match parse_target(&target_raw) {
                    Some(t) => (t, Relation::AlludesTo),
                    None => (node, Relation::Mentions),
                };
                let mut link = TypedLink::new(source, target, relation, Confidence::Possible);
                link.note = text;
                link.visibility = Visibility::Private;
                let _ = self::create_link(&slug, link).await;
                refresh.set(refresh() + 1);
                note_text.set(String::new());
                target_text.set(String::new());
                clip_in.set(None);
            }
        });
    });

    // Save this video to the library as a referenceable `type: video` note.
    let save_v = v.clone();
    let save_lib = use_callback(move |()| {
        let slug = selected_slugs(&selection.read(), &org_list.read())
            .first()
            .cloned();
        let id = save_v.clone();
        spawn(async move {
            if let Some(slug) = slug {
                let url = format!("https://youtu.be/{id}");
                let _ = self::save_video_note(&slug, &id, &url, "").await;
                saved.set(true);
            }
        });
    });

    let moment_list = match &*moments.read() {
        Some(Some(ms)) if !ms.is_empty() => rsx! {
            div { class: "flex flex-col divide-y divide-border/40",
                // Index-qualified key: two notes can anchor the same second.
                for (i, m) in ms.clone().into_iter().enumerate() {
                    div { key: "{i}-{m.start}", class: "group flex items-start gap-2 py-2",
                        button {
                            r#type: "button",
                            class: "mt-0.5 shrink-0 rounded-md border border-border/70 bg-card/60 px-1.5 py-0.5 font-mono text-[0.7rem] text-muted-foreground hover:border-primary/60 hover:text-foreground",
                            title: "Seek to {m.label}",
                            onclick: move |_| seek_embed(m.start),
                            "{m.label}"
                        }
                        div { class: "flex min-w-0 flex-col",
                            if !m.note.is_empty() {
                                span { class: "text-sm leading-snug text-foreground", "{m.note}" }
                            }
                            if let Some(t) = &m.target {
                                span { class: "text-xs text-muted-foreground", "→ {t}" }
                            }
                        }
                    }
                }
            }
        },
        Some(Some(_)) => rsx! {
            Text { variant: TextVariant::Muted, class: "py-4", "No notes yet — scrub to a moment and add one below." }
        },
        _ => rsx! {
            Text { variant: TextVariant::Muted, class: "py-4", "Loading notes…" }
        },
    };

    // Synced transcript: click a line to jump there and pre-fill the note.
    let transcript_panel = match &*transcript.read() {
        Some(Some(segs)) if !segs.is_empty() => {
            let segs = segs.clone();
            rsx! {
                section { class: "flex flex-col gap-1",
                    div { class: "text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground",
                        "Transcript · {segs.len()} lines"
                    }
                    div { class: "max-h-96 overflow-y-auto rounded-xl border border-border/70 bg-card/20 p-2",
                        for (i, seg) in segs.into_iter().enumerate() {
                            {
                                let start = seg.start as u32;
                                let stamp = format_timecode(start);
                                let text = seg.text;
                                let shown = text.clone();
                                rsx! {
                                    div {
                                        key: "{i}",
                                        class: "group flex cursor-pointer items-start gap-2 rounded px-1 py-0.5 hover:bg-accent/30",
                                        onclick: move |_| {
                                            seek_embed(start);
                                            note_text.set(text.clone());
                                        },
                                        span { class: "mt-0.5 shrink-0 font-mono text-[0.65rem] text-muted-foreground group-hover:text-foreground",
                                            "{stamp}"
                                        }
                                        span { class: "text-xs leading-snug text-muted-foreground group-hover:text-foreground",
                                            "{shown}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => rsx! {},
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-4xl flex-col gap-4 p-4 sm:p-6 lg:p-8",
            header { class: "flex items-baseline justify-between gap-3",
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Watch" }
                div { class: "flex items-center gap-3",
                    button {
                        r#type: "button",
                        class: "text-xs text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground",
                        disabled: saved(),
                        onclick: move |_| save_lib.call(()),
                        if saved() { "Saved to library ✓" } else { "Save to library" }
                    }
                    button {
                        r#type: "button",
                        class: "text-xs text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground",
                        onclick: move |_| { nav.push(task_plugin_ui::href(APP_ID, "", "")); },
                        "Open another video"
                    }
                }
            }
            iframe {
                id: EMBED_ID,
                class: "aspect-video w-full rounded-xl border border-border/70 bg-black",
                src: "https://www.youtube.com/embed/{v}?enablejsapi=1",
                allow: "autoplay; encrypted-media; picture-in-picture",
                allowfullscreen: true,
            }
            // Add a note / clip at the current time.
            div { class: "flex flex-col gap-2 rounded-xl border border-border/70 bg-card/30 p-2",
                div { class: "flex items-center gap-2",
                    input {
                        r#type: "text",
                        class: "h-9 flex-1 rounded-md border border-border bg-background px-2.5 text-sm",
                        placeholder: "Note at the current moment…",
                        value: "{note_text}",
                        oninput: move |e| note_text.set(e.value()),
                        onkeydown: move |e| if e.key() == Key::Enter { add_note.call(()) },
                    }
                    Button {
                        on_click: move |_| add_note.call(()),
                        if clip_in().is_some() { "Add clip" } else { "Add at current time" }
                    }
                }
                div { class: "flex items-center gap-2",
                    input {
                        r#type: "text",
                        class: "h-8 flex-1 rounded-md border border-border bg-background px-2.5 text-xs",
                        placeholder: "Link to (optional): John.3.16  ·  a-topic  ·  verse:Rom.5.8",
                        value: "{target_text}",
                        oninput: move |e| target_text.set(e.value()),
                    }
                    if let Some(secs) = clip_in() {
                        button {
                            r#type: "button",
                            class: "rounded-md border border-primary/40 bg-primary/10 px-2 py-1 text-xs text-foreground hover:bg-primary/20",
                            title: "Clear clip start",
                            onclick: move |_| clip_in.set(None),
                            "clip from {format_timecode(secs)} ✕"
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "rounded-md border border-border bg-background px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                            title: "Mark the clip start at the current moment",
                            onclick: move |_| mark_in.call(()),
                            "Mark in"
                        }
                    }
                }
            }
            section { class: "flex flex-col gap-1",
                div { class: "text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground",
                    "Notes · {node_token}"
                }
                {moment_list}
            }
            {transcript_panel}
        }
    }
}

/// The no-video landing: paste a YouTube URL to start.
#[component]
fn UrlPrompt() -> Element {
    let nav = use_navigator();
    let mut url = use_signal(String::new);
    let go = use_callback(move |()| {
        if let Some(id) = youtube_id(&url()) {
            // Two parameters, so each value is encoded on its own —
            // `href` cannot tell this app.s `&` from the URL.s.
            nav.push(task_plugin_ui::href(
                APP_ID,
                "",
                &format!(
                    "v={}&node={}",
                    task_plugin_ui::encode(&id),
                    task_plugin_ui::encode(&format!("video:{id}"))
                ),
            ));
        }
    });
    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-xl flex-col gap-4 p-6 sm:p-8",
            Heading { level: HeadingLevel::H1, class: "tracking-tight", "Watch" }
            Text { variant: TextVariant::Muted,
                "Paste a YouTube link to watch it and take timestamped notes."
            }
            div { class: "flex items-center gap-2",
                input {
                    r#type: "text",
                    class: "h-10 flex-1 rounded-md border border-border bg-background px-3 text-sm",
                    placeholder: "https://youtu.be/…",
                    value: "{url}",
                    oninput: move |e| url.set(e.value()),
                    onkeydown: move |e| if e.key() == Key::Enter { go.call(()) },
                }
                Button { on_click: move |_| go.call(()), "Watch" }
            }
        }
    }
}

/// What this crate makes of a note.
///
/// A `type: video` note **is** the watch screen — the video, its
/// transcript, and the timestamped notes hanging off it. The shell used
/// to special-case that (`else if is_video`, right next to the widget
/// registry it was bypassing), which meant the one place that decides
/// what a note looks like had an exception carved into it for one note
/// type belonging to one app.
///
/// The node token is derived from the note's own basename, the same way
/// the shell derived it, so `Videos/dQw4w9WgXcQ.md` watches
/// `video:dQw4w9WgXcQ`.
#[must_use]
pub fn widgets() -> Vec<task_plugin_ui::task_widgets::WidgetSpec> {
    use task_plugin_ui::task_widgets::{WidgetMatch, WidgetSpec};
    vec![
        WidgetSpec::new("studio.video", vec![WidgetMatch::NoteType("video")])
            .render(|ctx| {
                let id = basename_of(&ctx.path).to_string();
                rsx! {
                    WatchView { v: id.clone(), node: format!("video:{id}") }
                }
            })
            .plugin(APP_ID),
    ]
}

/// A note's basename without its extension — `Videos/abc.md` → `abc`.
fn basename_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name)
}
