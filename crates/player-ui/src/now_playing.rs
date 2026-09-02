//! The global **Now Playing** mini-player.
//!
//! Split in two so the music survives navigation while the UI lives in the
//! status bar:
//!
//! - [`GlobalNowPlayer`] is the headless engine — mounted once in
//!   [`the app shell`], OUTSIDE the route `Outlet`, so
//!   its single `<audio>` element keeps playing across route changes. It
//!   owns the queue (captured at play time, see
//!   [`crate::context::NowPlayingRequest`]), mirrors its state into
//!   [`NowPlayingCtl`], and executes transport commands the UI posts back.
//!   It renders nothing.
//! - [`NowPlayingTab`] is the UI — a small rounded tab docked in the
//!   bottom-right of the IDE status bar (poking up a few px above the status
//!   line), expanding on hover to reveal prev/next + a scrubber. It reads
//!   [`NowPlayingCtl`] and posts transport commands.
//!
//! It plays each song's REFERENCE stem (one `<audio>` at a time, streamed
//! off disk from `/org/{org}/media/songs/{slug}/{file}`), reusing the
//! setlist stream player's `Track`/`load_tracks`/`element_for`. The
//! multitrack rehearsal rig and the per-song page stay separate.
//!
//! Future: a `video:` queue swaps the `<audio>` for a `<video>`.

use dioxus::prelude::*;
use task_ui_core::format::duration_mmss;

/// A transport command the [`NowPlayingTab`] UI posts to the headless
/// [`GlobalNowPlayer`] engine. `Seek` carries a 0..1 fraction of duration.
#[derive(Clone, Copy, PartialEq)]
pub enum NpCmd {
    Toggle,
    Next,
    Prev,
    Seek(f64),
    /// Pause if currently playing (a no-op otherwise). Used by the fullscreen
    /// setlist player to stop the global stream when it starts its own audio.
    Pause,
}

/// Shared control surface between the engine and the status-bar UI. The
/// engine WRITES the view signals and READS `cmd`; the tab READS the view
/// and WRITES `cmd`. Provided once at the app shell so both (siblings) see
/// the same instance.
#[derive(Clone, Copy)]
pub struct NowPlayingCtl {
    /// Current track title. `None` ⇒ nothing playing ⇒ the tab hides.
    pub track_title: Signal<Option<String>>,
    /// Queue label, e.g. `"Sunday Worship · 1/6"`.
    pub queue_label: Signal<String>,
    pub playing: Signal<bool>,
    /// Progress 0..1.
    pub frac: Signal<f64>,
    pub pos: Signal<f64>,
    pub dur: Signal<f64>,
    pub can_prev: Signal<bool>,
    pub can_next: Signal<bool>,
    /// Slug of the currently playing song, so the setlist list can mark the
    /// matching row (`None` ⇒ nothing playing).
    pub current_slug: Signal<Option<String>>,
    /// Live output amplitude 0..1 (smoothed), driving the now-playing
    /// waveform over the row artwork.
    pub amp: Signal<f64>,
    /// Command bus: `(generation, cmd)` — a bumped generation makes repeats
    /// observable.
    pub cmd: Signal<(u64, NpCmd)>,
}

/// Install [`NowPlayingCtl`]. Call once in the app shell, above both the
/// engine and the status bar.
pub fn provide_now_playing_ctl() {
    use_context_provider(|| NowPlayingCtl {
        track_title: Signal::new(None),
        queue_label: Signal::new(String::new()),
        playing: Signal::new(false),
        frac: Signal::new(0.0),
        pos: Signal::new(0.0),
        dur: Signal::new(0.0),
        can_prev: Signal::new(false),
        can_next: Signal::new(false),
        current_slug: Signal::new(None),
        amp: Signal::new(0.0),
        cmd: Signal::new((0, NpCmd::Toggle)),
    });
}

/// The bottom-right status-bar tab. Renders nothing until something plays.
/// Collapsed it's just play/pause + title + a hairline progress bar; on
/// hover it expands to prev/next + a scrubber + times.
#[component]
pub fn NowPlayingTab() -> Element {
    let ctl = use_context::<NowPlayingCtl>();
    let title = ctl.track_title.read().clone();
    let Some(title) = title else {
        return rsx! {};
    };
    let label = ctl.queue_label.read().clone();
    let playing = (ctl.playing)();
    let frac = (ctl.frac)();
    let pos = (ctl.pos)();
    let dur = (ctl.dur)();
    let can_prev = (ctl.can_prev)();
    let can_next = (ctl.can_next)();
    let cmd = ctl.cmd;
    let send = move |c: NpCmd| {
        let mut cmd = cmd; // local Copy of the signal handle → `send` stays `Fn`
        let g = cmd.peek().0 + 1;
        cmd.set((g, c));
    };

    rsx! {
        // Wide mini-player tab (Spotify-desktop-bar style), single row:
        // album art · title/subtitle · a stretchy custom seek bar · time ·
        // spaced-out SVG transport with a proper play button. ~2× the
        // status-bar height, rounded top, poking up out of the status line.
        div {
            class: "flex h-12 w-[40rem] items-center gap-3 rounded-t-lg border border-b-0 border-border bg-card/95 px-3 shadow-md backdrop-blur",
            title: "{label}",
            // ── playback controls (left) — spaced-out SVG transport ──
            div { class: "flex shrink-0 items-center gap-3",
                button {
                    r#type: "button",
                    class: "text-muted-foreground hover:text-foreground disabled:opacity-30",
                    disabled: !can_prev,
                    onclick: move |_| send(NpCmd::Prev),
                    svg { view_box: "0 0 24 24", fill: "currentColor", class: "h-3.5 w-3.5",
                        path { d: "M7 6h2v12H7zM19 6l-9 6 9 6z" }
                    }
                }
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 items-center justify-center rounded-full bg-primary text-primary-foreground shadow transition-transform hover:scale-105 active:scale-95",
                    onclick: move |_| send(NpCmd::Toggle),
                    if playing {
                        svg { view_box: "0 0 24 24", fill: "currentColor", class: "h-4 w-4",
                            path { d: "M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" }
                        }
                    } else {
                        svg { view_box: "0 0 24 24", fill: "currentColor", class: "h-4 w-4 translate-x-px",
                            path { d: "M8 5v14l11-7z" }
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "text-muted-foreground hover:text-foreground disabled:opacity-30",
                    disabled: !can_next,
                    onclick: move |_| send(NpCmd::Next),
                    svg { view_box: "0 0 24 24", fill: "currentColor", class: "h-3.5 w-3.5",
                        path { d: "M15 6h2v12h-2zM5 6l9 6-9 6z" }
                    }
                }
            }
            // ── elapsed time ──
            span { class: "shrink-0 text-[10px] tabular-nums text-muted-foreground", "{duration_mmss(pos)}" }
            // ── stretchy custom seek bar (middle) ──
            div { class: "group/seek relative flex h-2.5 min-w-0 flex-1 items-center",
                div { class: "h-1 w-full overflow-hidden rounded-full bg-muted",
                    div { class: "h-full rounded-full bg-primary", style: "width: {frac * 100.0}%" }
                }
                div {
                    class: "pointer-events-none absolute top-1/2 h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-primary opacity-0 shadow transition-opacity group-hover/seek:opacity-100",
                    style: "left: {frac * 100.0}%",
                }
                input {
                    r#type: "range",
                    min: "0",
                    max: "1000",
                    value: "{(frac * 1000.0) as i64}",
                    class: "absolute inset-0 h-full w-full cursor-pointer opacity-0",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f64>() {
                            send(NpCmd::Seek(v / 1000.0));
                        }
                    },
                }
            }
            // ── total time ──
            span { class: "shrink-0 text-[10px] tabular-nums text-muted-foreground", "{duration_mmss(dur)}" }
            // ── song details (right): album art + title/subtitle ──
            div { class: "flex shrink-0 items-center gap-2",
                div { class: "min-w-0 text-right leading-tight",
                    div { class: "max-w-[10rem] truncate text-xs font-semibold text-foreground", "{title}" }
                    div { class: "max-w-[10rem] truncate text-[10px] text-muted-foreground", "{label}" }
                }
                div { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-gradient-to-br from-primary/80 to-primary/30 text-sm font-bold text-primary-foreground shadow-sm",
                    "{title.chars().next().unwrap_or('♪')}"
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{AnalyserNode, AudioContext, HtmlAudioElement, MediaElementAudioSourceNode};

    use super::{NowPlayingCtl, NpCmd};
    use crate::context::NowPlaying;
    use crate::setlist_stream::imp::{Track, element_for, load_tracks};

    /// Headless engine: owns the audio + queue, mirrors state to
    /// [`NowPlayingCtl`], and runs transport commands. Renders nothing (the
    /// UI is [`super::NowPlayingTab`], in the status bar). Mounted by
    /// [`super::GlobalNowPlayer`] once something asks to play.
    #[component]
    pub fn NowPlayingEngine() -> Element {
        let element: Rc<RefCell<Option<HtmlAudioElement>>> =
            use_hook(|| Rc::new(RefCell::new(None)));
        // Web Audio graph for the now-playing waveform amplitude — created
        // lazily on first play. The source (per element) is kept alive here.
        let audio_ctx: Rc<RefCell<Option<(AudioContext, AnalyserNode)>>> =
            use_hook(|| Rc::new(RefCell::new(None)));
        let audio_src: Rc<RefCell<Option<MediaElementAudioSourceNode>>> =
            use_hook(|| Rc::new(RefCell::new(None)));
        let mut queue_key = use_signal(|| (String::new(), Vec::<String>::new()));
        let mut title = use_signal(String::new);
        let mut current = use_signal(|| None::<usize>);
        let mut playing = use_signal(|| false);
        let mut position = use_signal(|| 0.0f64);
        let mut duration = use_signal(|| 0.0f64);
        let mut pending_start = use_signal(|| None::<usize>);

        let tracks = use_resource(move || {
            let (org, songs) = queue_key();
            async move {
                if songs.is_empty() {
                    Vec::<Track>::new()
                } else {
                    load_tracks(&org, &songs).await.unwrap_or_default()
                }
            }
        });

        let select = use_callback({
            let element = element.clone();
            let audio_ctx = audio_ctx.clone();
            let audio_src = audio_src.clone();
            move |i: usize| {
                let track = {
                    let list = tracks.peek();
                    match list.as_ref().and_then(|l| l.get(i).cloned()) {
                        Some(t) => t,
                        None => return,
                    }
                };
                let Some(file) = track.reference.clone() else {
                    tracing::warn!("now-playing: `{}` has no reference stem", track.slug);
                    return;
                };
                let org = queue_key.peek().0.clone();
                if let Some(old) = element.borrow_mut().take() {
                    let _ = old.pause();
                }
                current.set(Some(i));
                playing.set(true);
                position.set(0.0);
                duration.set(track.duration_sec);
                match element_for(&org, &track.slug, &file) {
                    Ok(el) => {
                        let _ = el.play();
                        // Best-effort: route through a Web Audio analyser so the
                        // waveform can track amplitude. If anything fails the
                        // element still plays normally (no source is created).
                        {
                            let mut g = audio_ctx.borrow_mut();
                            if g.is_none() {
                                if let Ok(ctx) = AudioContext::new() {
                                    if let Ok(an) = ctx.create_analyser() {
                                        an.set_fft_size(256);
                                        an.set_smoothing_time_constant(0.75);
                                        let _ = an.connect_with_audio_node(&ctx.destination());
                                        *g = Some((ctx, an));
                                    }
                                }
                            }
                            if let Some((ctx, an)) = g.as_ref() {
                                let _ = ctx.resume();
                                if let Ok(src) = ctx.create_media_element_source(&el) {
                                    let _ = src.connect_with_audio_node(an);
                                    *audio_src.borrow_mut() = Some(src);
                                }
                            }
                        }
                        *element.borrow_mut() = Some(el);
                    }
                    Err(e) => tracing::warn!("now-playing: `{}`: {e}", track.slug),
                }
            }
        });

        let toggle = use_callback({
            let element = element.clone();
            move |()| {
                if current.peek().is_none() {
                    select.call(0);
                    return;
                }
                if let Some(el) = element.borrow().as_ref() {
                    if el.paused() {
                        let _ = el.play();
                        playing.set(true);
                    } else {
                        let _ = el.pause();
                        playing.set(false);
                    }
                }
            }
        });

        let seek = use_callback({
            let element = element.clone();
            move |frac: f64| {
                let dur = duration.peek().max(0.0);
                if dur <= 0.0 {
                    return;
                }
                let t = frac.clamp(0.0, 1.0) * dur;
                if let Some(el) = element.borrow().as_ref() {
                    el.set_current_time(t);
                    position.set(t);
                }
            }
        });

        // Play a pending start once the queue's tracks load.
        use_effect(move || {
            let ready = tracks
                .read()
                .as_ref()
                .map(|l| !l.is_empty())
                .unwrap_or(false);
            let pending = *pending_start.peek();
            if ready {
                if let Some(s) = pending {
                    pending_start.set(None);
                    select.call(s);
                }
            }
        });

        // Answer global play requests.
        {
            let req = use_context::<NowPlaying>().0;
            let mut last_gen = use_signal(|| 0u64);
            use_effect(move || {
                let r = req();
                if r.generation == 0 || r.generation == *last_gen.peek() {
                    return;
                }
                last_gen.set(r.generation);
                let same = {
                    let k = queue_key.peek();
                    k.0 == r.org && k.1 == r.songs
                };
                if same {
                    let ready = tracks
                        .peek()
                        .as_ref()
                        .map(|l| !l.is_empty())
                        .unwrap_or(false);
                    if ready {
                        if r.toggle {
                            toggle.call(());
                        } else {
                            select.call(r.start);
                        }
                    } else {
                        pending_start.set(Some(r.start));
                    }
                } else {
                    title.set(r.title.clone());
                    current.set(None);
                    pending_start.set(Some(r.start));
                    queue_key.set((r.org.clone(), r.songs.clone()));
                }
            });
        }

        // Run transport commands posted by the status-bar tab.
        {
            let ctl = use_context::<NowPlayingCtl>();
            let cmd = ctl.cmd;
            let mut last = use_signal(|| 0u64);
            use_effect(move || {
                let (g, c) = cmd();
                if g == 0 || g == *last.peek() {
                    return;
                }
                last.set(g);
                match c {
                    NpCmd::Toggle => toggle.call(()),
                    NpCmd::Next => {
                        let i = (*current.peek()).map(|i| i + 1).unwrap_or(0);
                        select.call(i);
                    }
                    NpCmd::Prev => {
                        let i = (*current.peek()).map(|i| i.saturating_sub(1)).unwrap_or(0);
                        select.call(i);
                    }
                    NpCmd::Seek(f) => seek.call(f),
                    NpCmd::Pause => {
                        if *playing.peek() {
                            toggle.call(());
                        }
                    }
                }
            });
        }

        // Mirror engine state → the shared control surface for the UI.
        {
            let ctl = use_context::<NowPlayingCtl>();
            use_effect(move || {
                let cur = current();
                let list = tracks.read();
                let len = queue_key.read().1.len();
                let qtitle = title();
                match cur.and_then(|i| list.as_ref().and_then(|l| l.get(i)).map(|t| (i, t.clone())))
                {
                    Some((i, t)) => {
                        let label = if len > 1 {
                            format!("{qtitle} · {}/{}", i + 1, len)
                        } else {
                            qtitle
                        };
                        ctl.track_title.clone().set(Some(t.title.clone()));
                        ctl.queue_label.clone().set(label);
                        ctl.can_prev.clone().set(i > 0);
                        ctl.can_next.clone().set(i + 1 < len);
                        ctl.current_slug
                            .clone()
                            .set(queue_key.read().1.get(i).cloned());
                    }
                    None => {
                        ctl.track_title.clone().set(None);
                        ctl.current_slug.clone().set(None);
                    }
                }
            });
            use_effect(move || {
                ctl.playing.clone().set(playing());
            });
            use_effect(move || {
                let d = duration();
                let p = position();
                ctl.pos.clone().set(p);
                ctl.dur.clone().set(d);
                ctl.frac.clone().set(if d > 0.0 {
                    (p / d).clamp(0.0, 1.0)
                } else {
                    0.0
                });
            });
        }

        // 300 ms poll: mirror position/duration, auto-advance on ended.
        {
            let element = element.clone();
            use_future(move || {
                let element = element.clone();
                async move {
                    loop {
                        architect::platform::sleep(std::time::Duration::from_millis(300)).await;
                        let (pos, dur, ended) = match element.borrow().as_ref() {
                            Some(el) => {
                                let d = el.duration();
                                (
                                    el.current_time(),
                                    if d.is_finite() { d } else { 0.0 },
                                    el.ended(),
                                )
                            }
                            None => continue,
                        };
                        position.set(pos);
                        if dur > 0.0 {
                            duration.set(dur);
                        }
                        if ended {
                            let len = tracks.peek().as_ref().map(|l| l.len()).unwrap_or(0);
                            let next = (*current.peek()).map(|i| i + 1).unwrap_or(0);
                            if next < len {
                                select.call(next);
                            } else {
                                playing.set(false);
                            }
                        }
                    }
                }
            });
        }

        // ~30 fps amplitude poll: RMS of the analyser's time-domain data →
        // a smoothed 0..1 level that drives the now-playing waveform.
        {
            let ctl = use_context::<NowPlayingCtl>();
            let audio_ctx = audio_ctx.clone();
            let mut amp = ctl.amp;
            use_future(move || {
                let audio_ctx = audio_ctx.clone();
                async move {
                    let mut smooth = 0.0f64;
                    let mut buf = [0u8; 128];
                    loop {
                        architect::platform::sleep(std::time::Duration::from_millis(33)).await;
                        let is_playing = *playing.peek();
                        let rms = if is_playing {
                            match audio_ctx.borrow().as_ref() {
                                Some((_, an)) => {
                                    an.get_byte_time_domain_data(&mut buf);
                                    let sum: f64 = buf
                                        .iter()
                                        .map(|&b| {
                                            let v = (b as f64 - 128.0) / 128.0;
                                            v * v
                                        })
                                        .sum();
                                    (sum / buf.len() as f64).sqrt()
                                }
                                None => 0.0,
                            }
                        } else {
                            0.0
                        };
                        // Boost (stems sit well below 0 dBFS) + asymmetric smooth
                        // (fast attack, slow release) for a lively-but-stable bar.
                        let target = (rms * 3.2).min(1.0);
                        smooth = if target > smooth {
                            smooth * 0.4 + target * 0.6
                        } else {
                            smooth * 0.82 + target * 0.18
                        };
                        amp.set(smooth);
                    }
                }
            });
        }

        // Headless — the UI is the status-bar tab.
        rsx! {}
    }

    /// Headless: marks whichever setlist song-row matches the currently
    /// playing song with `md-song-strip--playing` and feeds it the live
    /// amplitude via a `--amp` custom property (driving the waveform over the
    /// row artwork). The rows are editor-rendered HTML, so this reconciles the
    /// DOM directly rather than through rsx.
    #[component]
    pub fn NowPlayingStripHighlighter() -> Element {
        let ctl = use_context::<NowPlayingCtl>();
        use_future(move || async move {
            loop {
                architect::platform::sleep(std::time::Duration::from_millis(33)).await;
                let active = if *ctl.playing.peek() {
                    ctl.current_slug.read().clone()
                } else {
                    None
                };
                let amp = *ctl.amp.peek();
                mark_playing_rows(active.as_deref(), amp);
            }
        });
        rsx! {}
    }

    /// Reconcile the `md-song-strip--playing` class + `--amp` across all
    /// on-screen song rows against the active slug.
    fn mark_playing_rows(active_slug: Option<&str>, amp: f64) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Ok(rows) = doc.query_selector_all(".md-song-strip") else {
            return;
        };
        let amp_str = format!("{amp:.3}");
        for i in 0..rows.length() {
            let Some(el) = rows
                .item(i)
                .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                continue;
            };
            let is_active = el
                .get_attribute("data-href")
                .and_then(|h| {
                    h.strip_prefix("song-play:")
                        .map(task_ui_core::frontmatter::slugify)
                })
                .as_deref()
                == active_slug;
            if is_active {
                let _ = el.class_list().add_1("md-song-strip--playing");
                let _ = el.style().set_property("--amp", &amp_str);
            } else {
                let _ = el.class_list().remove_1("md-song-strip--playing");
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::{NowPlayingEngine, NowPlayingStripHighlighter};

#[cfg(not(target_arch = "wasm32"))]
mod stub {
    use dioxus::prelude::*;

    /// Server/native build: the engine runs in the browser only.
    #[component]
    pub fn NowPlayingEngine() -> Element {
        rsx! {}
    }

    #[component]
    pub fn NowPlayingStripHighlighter() -> Element {
        rsx! {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use stub::{NowPlayingEngine, NowPlayingStripHighlighter};

/// The shell's mount point for the global player — outside the route
/// `Outlet`, so playback survives navigation.
///
/// Renders nothing until the first play request arrives, then mounts
/// the engine and the setlist-row highlighter. Two reasons for the
/// gate. Most sessions never play anything, so the engine's Web Audio
/// graph, its polling futures and its analyser should not exist for
/// them. And on the split web build the engine is its own chunk
/// ([`task_plugin_ui::lazy_element!`]): the request is what downloads
/// it, and the engine's own mount effect then reads that same request
/// — its `last_gen` starts at zero — so nothing is lost in the gap.
#[component]
pub fn GlobalNowPlayer() -> Element {
    let requests = use_context::<crate::context::NowPlaying>().0;
    if requests.read().generation == 0 {
        return rsx! {};
    }
    task_plugin_ui::lazy_element!("player_engine", player_engine)
}

/// What the engine chunk contains: the headless player and the
/// highlighter that follows it through the DOM.
fn player_engine() -> Element {
    rsx! {
        NowPlayingEngine {}
        NowPlayingStripHighlighter {}
    }
}
