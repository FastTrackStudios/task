//! The player's note-widget specs — the setlist/song embed the Task
//! shell used to hand-wire in `note_view.rs`, now provided through the
//! `task-widgets` registry.
//!
//! Three specs:
//!
//! - `player.song` (`type: song`): a song note IS its player. On mount
//!   it drops straight into the fullscreen experience; `Esc`/minimize
//!   falls back to the compact streaming card ([`SongCard`]).
//! - `player.setlist` (`type: setlist`, or `experience: setlist` on any
//!   note): the fullscreen setlist experience while
//!   [`WidgetCtx::fullscreen`] is set. The *embedded* view mounts
//!   nothing — the visible header + song strips are the editor's own
//!   setlist-title/song-strip decorations, whose ▶/Open clicks arrive
//!   as hrefs.
//! - `player.embed` (a note embedding `type: song` / `type: setlist`
//!   targets via standalone wikilinks): href handling only, so an event
//!   note's embedded set still gets a working queue.
//!
//! All three share [`player_href`]: `song-play:` / `song-more:` /
//! `setlist-open:` / `setlist-play:` clicks from the editor's link
//! channel. Playback goes to the GLOBAL Now Playing player (mounted in
//! the app shell, so it survives navigation) via the [`NowPlaying`]
//! context; the queue is resolved from the live note text + vault
//! resolver at click time.

use dioxus::prelude::*;
use task_ui_core::frontmatter::{
    frontmatter_value, setlist_song_links_from_body, setlist_songs_from, slugify, song_slug_from,
};
use task_widgets::{FullscreenExperience, WidgetCtx, WidgetMatch, WidgetSpec, WidgetTarget};

use crate::context::{NowPlaying, NowPlayingRequest};

/// The player's widget specs — the `fasttrackstudio` plugin's widget
/// contribution, registered (per provider) at the app root.
#[must_use]
pub fn widgets() -> Vec<WidgetSpec> {
    // The renders are lazy: what a song or setlist note *is* (the
    // matches, the href handling) stays in the shell, but the player
    // they mount — the multitrack Web Audio graph, the daw worklet,
    // the engraved chart pane — downloads the first time one opens.
    // ONE boundary for both specs — a second `lazy_render!` would be a
    // second split point with its own copy of the player (see
    // `player_note_widget`).
    let player_render = task_plugin_ui::lazy_render!("player_widget", player_note_widget);
    vec![
        WidgetSpec::new("player.song", vec![WidgetMatch::NoteType("song")])
            .render(player_render)
            .on_href(player_href)
            .fullscreen_owns_body()
            .plugin("fasttrackstudio"),
        WidgetSpec::new(
            "player.setlist",
            vec![
                WidgetMatch::NoteType("setlist"),
                WidgetMatch::NoteExperience("setlist"),
            ],
        )
        .render(player_render)
        .on_href(player_href)
        // The editor's typed setlist-title widget IS the title.
        .hide_note_header()
        .fullscreen_owns_body()
        .plugin("fasttrackstudio"),
        WidgetSpec::new(
            "player.embed",
            vec![
                WidgetMatch::EmbedType("song"),
                WidgetMatch::EmbedType("setlist"),
            ],
        )
        .on_href(player_href)
        .plugin("fasttrackstudio"),
    ]
}

/// Whether the host note is setlist-shaped: `type: setlist`, or an
/// explicit `experience: setlist` frontmatter opt-in on any note.
fn is_setlist_note(ctx: &WidgetCtx, doc: &str) -> bool {
    let type_is = matches!(
        &ctx.target,
        WidgetTarget::Note { note_type } if note_type.as_deref() == Some("setlist")
    );
    type_is
        || frontmatter_value(doc, "experience")
            .map(|v| v.trim().trim_matches(['"', '\'']).trim() == "setlist")
            .unwrap_or(false)
}

/// The queue a note's play clicks belong to, resolved at click time:
/// a setlist-shaped note's own songs; otherwise the IMPLICIT setlist —
/// `type: song` wikilinks directly in the note (an event IS its setlist,
/// no separate setlist doc required), falling back to the first EMBEDDED
/// `type: setlist` note's songs.
fn queue_songs(ctx: &WidgetCtx, doc: &str) -> Vec<String> {
    if is_setlist_note(ctx, doc) {
        return setlist_songs_from(doc);
    }
    let links = setlist_song_links_from_body(doc);
    let kind_of = |target: &str| (ctx.resolve)(target).and_then(|r| r.note_type);
    let direct: Vec<String> = links
        .iter()
        .filter(|t| kind_of(t).as_deref() == Some("song"))
        .map(|t| slugify(t))
        .collect();
    if !direct.is_empty() {
        return direct;
    }
    links
        .iter()
        .find_map(|target| {
            let resolved = (ctx.resolve)(target)?;
            if resolved.note_type.as_deref() != Some("setlist") {
                return None;
            }
            resolved.content.map(|raw| setlist_songs_from(&raw))
        })
        .unwrap_or_default()
}

/// Post a request to the global Now Playing player.
fn request_now_playing(ctx: &WidgetCtx, songs: Vec<String>, start: usize, toggle: bool) {
    let Some(now_playing) = try_consume_context::<NowPlaying>() else {
        tracing::warn!("player widget: NowPlaying context missing (provide_player_contexts?)");
        return;
    };
    let mut sig = now_playing.0;
    let generation = sig.peek().generation + 1;
    sig.set(NowPlayingRequest {
        generation,
        org: ctx.org.clone(),
        title: ctx.title.clone(),
        songs,
        start,
        toggle,
    });
}

/// The player's editor-link handler (self-gating on its href schemes).
fn player_href(href: &str, ctx: &WidgetCtx) -> bool {
    if let Some(name) = href.strip_prefix("song-play:") {
        // Play within the note's queue; a lone strip becomes a 1-song queue.
        let slug = slugify(name);
        let queue = queue_songs(ctx, &(ctx.doc)());
        let songs = if queue.is_empty() {
            vec![slug.clone()]
        } else {
            queue
        };
        let start = songs.iter().position(|s| *s == slug).unwrap_or(0);
        request_now_playing(ctx, songs, start, false);
        return true;
    }
    if let Some(name) = href.strip_prefix("song-more:") {
        // "…" more-actions on a song row → open the song note (its full
        // action surface). A dedicated inline menu is a follow-up.
        let page = name.split(['#', '|']).next().unwrap_or(name).trim();
        let path = (ctx.resolve)(page)
            .map(|r| r.path)
            .unwrap_or_else(|| format!("{page}.md"));
        ctx.open_note.call(path);
        return true;
    }
    if href.starts_with("setlist-open:") {
        // Open the full-screen setlist experience (the button lives in the
        // editor's setlist-title widget, so it travels with an embedded
        // setlist too).
        let mut fullscreen = ctx.fullscreen;
        fullscreen.set(true);
        return true;
    }
    if href.starts_with("setlist-play:") {
        // Header ▶: start the whole setlist, or toggle if it's already
        // the loaded queue.
        let songs = queue_songs(ctx, &(ctx.doc)());
        request_now_playing(ctx, songs, 0, true);
        return true;
    }
    false
}

/// The song and setlist block renders, as ONE chunk boundary.
///
/// One function for both specs, deliberately: the splitter puts what a
/// split point reaches and the main module does not into that point's
/// own module, and code two split points share is not pooled — a song
/// render and a setlist render would each carry their own copy of the
/// whole player (12 MB apiece, measured). Which experience to mount is
/// the same question the specs' matches already answered, asked again
/// of the note: setlist-shaped, or a song.
fn player_note_widget(ctx: WidgetCtx) -> Element {
    if is_setlist_note(&ctx, &(ctx.doc)()) {
        rsx! { SetlistNoteWidget { ctx } }
    } else {
        rsx! { SongNoteWidget { ctx } }
    }
}

/// The `type: song` note view. A song IS its player: on mount it drops
/// straight into the full-screen experience (the same immersive view as
/// a one-song set); `Esc`/minimize falls back to the compact streaming
/// card, which stays minimized until the next note (this widget mounts
/// fresh per note — the host note view is keyed by pane:path).
#[component]
fn SongNoteWidget(ctx: WidgetCtx) -> Element {
    let mut fullscreen = ctx.fullscreen;
    // Auto-fullscreen exactly once per mount ("armed" so a later
    // minimize isn't fought by this effect).
    let mut armed = use_signal(|| true);
    use_effect(move || {
        if *armed.peek() {
            armed.set(false);
            fullscreen.set(true);
        }
    });
    let doc = (ctx.doc)();
    let slug = song_slug_from(&doc, &ctx.title);
    if fullscreen() {
        rsx! {
            FullscreenExperience {
                title: ctx.title.clone(),
                on_exit: move |_| fullscreen.set(false),
                crate::SetlistPlayer {
                    songs: vec![slug.clone()],
                    org: ctx.org.clone(),
                    fullscreen: true,
                }
            }
        }
    } else {
        rsx! {
            SongCard {
                title: ctx.title.clone(),
                on_play: {
                    let ctx = ctx.clone();
                    let slug = slug.clone();
                    move |_| request_now_playing(&ctx, vec![slug.clone()], 0, true)
                },
                on_open: move |_| fullscreen.set(true),
            }
        }
    }
}

/// The `type: setlist` note view: the full-screen Experience while the
/// host's fullscreen signal is set; the embedded view mounts nothing
/// (the note's own editor widgets are the visible setlist — their ▶ and
/// Open clicks arrive through [`player_href`]).
#[component]
fn SetlistNoteWidget(ctx: WidgetCtx) -> Element {
    let mut fullscreen = ctx.fullscreen;
    if !fullscreen() {
        return rsx! {};
    }
    let songs = setlist_songs_from(&(ctx.doc)());
    rsx! {
        FullscreenExperience {
            title: ctx.title.clone(),
            on_exit: move |_| fullscreen.set(false),
            crate::SetlistPlayer { songs, org: ctx.org.clone(), fullscreen: true }
        }
    }
}

/// The compact, Apple-Music-style card for a song note in its minimized
/// (embedded) state: artwork tile + title / artist + a Play button (drives the
/// global Now Playing stream) and an "Open" that launches the full-screen
/// player experience. The title splits on `" - "` (`Praise - Elevation
/// Worship` → title `Praise`, artist `Elevation Worship`).
#[component]
fn SongCard(title: String, on_play: EventHandler<()>, on_open: EventHandler<()>) -> Element {
    let (name, artist) = match title.split_once(" - ") {
        Some((t, a)) => (t.trim().to_string(), a.trim().to_string()),
        None => (title.clone(), String::new()),
    };
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "♪".to_string());
    rsx! {
        div { class: "mx-4 my-4 flex items-center gap-3 rounded-xl border border-border bg-card px-3 py-2.5 shadow-sm",
            // Artwork tile (initial placeholder — real art slots in later).
            div { class: "flex size-12 shrink-0 items-center justify-center rounded-md bg-gradient-to-br from-primary/70 to-primary text-lg font-bold text-primary-foreground",
                "{initial}"
            }
            div { class: "min-w-0 flex-1",
                div { class: "truncate text-sm font-semibold text-foreground", "{name}" }
                if !artist.is_empty() {
                    div { class: "truncate text-xs text-muted-foreground", "{artist}" }
                }
            }
            // Play → global Now Playing stream.
            button {
                class: "flex size-9 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground transition-colors hover:bg-primary/90",
                title: "Play",
                onclick: move |_| on_play.call(()),
                svg {
                    view_box: "0 0 24 24",
                    fill: "currentColor",
                    class: "size-4 translate-x-[1px]",
                    path { d: "M8 5v14l11-7z" }
                }
            }
            // Open → the full-screen player experience.
            button {
                class: "shrink-0 rounded-md border border-border px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                onclick: move |_| on_open.call(()),
                "Open"
            }
        }
    }
}

/// Install the Keyflow chart renderer into the editor's fence registry.
///
/// `editor-state` renders ```` ```kf ```` fences through a registry rather
/// than a direct dependency — depending on the chart renderer would put the
/// whole editor stack above the notation domain and stop the editor being
/// embeddable on its own. Nothing registered means charts fall back to
/// showing their source, so this is what makes them engrave.
///
/// Exposed here because `editor-keyflow` is this crate's business, not the
/// shell's. Call once, at the app root, alongside the widget roster.
///
/// In the split web build what gets registered is [`lazy_fences::LazyFences`]:
/// the engraver and its notation fonts download the first time a chart
/// fence is actually rendered, not at boot.
pub fn register_chart_fences() {
    editor_state::fence_renderer::register_fence_renderer("kf", std::sync::Arc::new(fences()));
}

#[cfg(not(all(target_arch = "wasm32", feature = "wasm-split")))]
fn fences() -> editor_keyflow::Fences {
    editor_keyflow::Fences
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-split"))]
fn fences() -> lazy_fences::LazyFences {
    lazy_fences::LazyFences
}

/// The chart engraver behind a chunk boundary.
///
/// `FenceRenderer` is synchronous — the editor asks for an SVG in the
/// middle of its decoration pass and wants the answer now — so a
/// renderer whose code has not downloaded yet can only decline. It
/// does, and it does two more things: it starts the download, and it
/// *reads a signal* while declining. The decoration pass runs inside a
/// Dioxus effect, so that read subscribes the pass; when the chunk
/// lands and the signal flips, the pass re-runs and this time the
/// engraver answers. Between the two, the fence shows its source — the
/// same thing an unknown fence language shows, and honest about what
/// has happened.
#[cfg(all(target_arch = "wasm32", feature = "wasm-split"))]
mod lazy_fences {
    use std::sync::atomic::{AtomicBool, Ordering};

    use dioxus::prelude::*;
    use editor_state::fence_renderer::FenceRenderer;

    /// One call into the chunk carries either question, so the engraver
    /// is a single split point and one download.
    pub enum FenceRequest {
        Svg(String),
        Highlight(String),
    }

    pub enum FenceReply {
        Svg(Option<String>),
        Html(String),
    }

    /// The engraver proper — everything `editor_keyflow::Fences`
    /// reaches lives in this function's chunk.
    fn engrave_fence(req: FenceRequest) -> FenceReply {
        let fences = editor_keyflow::Fences;
        match req {
            FenceRequest::Svg(source) => FenceReply::Svg(fences.render_svg(&source)),
            FenceRequest::Highlight(source) => FenceReply::Html(fences.highlight_html(&source)),
        }
    }

    static ENGRAVER: dioxus::wasm_split::LazyLoader<FenceRequest, FenceReply> = {
        use dioxus::wasm_split;
        wasm_split::lazy_loader!(extern "engraver" fn engrave_fence(req: FenceRequest) -> FenceReply)
    };

    /// Flips once the chunk is here. Read by every declined render so
    /// the pass that declined re-runs.
    static ENGRAVER_READY: GlobalSignal<bool> = GlobalSignal::new(|| false);
    /// The download is started once, by whichever fence asks first.
    static STARTED: AtomicBool = AtomicBool::new(false);

    /// Is the engraver callable? Starts the download the first time it
    /// is not, and subscribes the asking reactive context to the flip.
    fn ensure_engraver() -> bool {
        // Outside a Dioxus runtime (nothing renders fences there, but a
        // registry lookup could happen anywhere) there is no signal to
        // read and nothing to spawn on: just decline.
        if dioxus::core::Runtime::try_current().is_none() {
            return false;
        }
        if ENGRAVER_READY() {
            return true;
        }
        if !STARTED.swap(true, Ordering::AcqRel) {
            // On the root scope, so the download outlives whichever
            // note happened to hold the first chart.
            dioxus::core::spawn_forever(async move {
                if ENGRAVER.load().await {
                    *ENGRAVER_READY.write() = true;
                } else {
                    tracing::warn!("chart fences: the engraver chunk did not download");
                }
            });
        }
        false
    }

    pub struct LazyFences;

    impl FenceRenderer for LazyFences {
        fn render_svg(&self, source: &str) -> Option<String> {
            if !ensure_engraver() {
                return None;
            }
            match ENGRAVER.call(FenceRequest::Svg(source.to_owned())) {
                Ok(FenceReply::Svg(svg)) => svg,
                _ => None,
            }
        }

        fn highlight_html(&self, source: &str) -> String {
            if ensure_engraver() {
                if let Ok(FenceReply::Html(html)) =
                    ENGRAVER.call(FenceRequest::Highlight(source.to_owned()))
                {
                    return html;
                }
            }
            escape_html(source)
        }
    }

    /// The editor's own fallback for a language it cannot highlight:
    /// the source, escaped.
    fn escape_html(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        for ch in source.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c => out.push(c),
            }
        }
        out
    }
}
