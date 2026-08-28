//! ONE playback system, two zoom levels.
//!
//! Media the app plays through the review surface — an audio master, a
//! video cut — is hosted HERE, at the shell, not on the page that
//! started it. The full review screen is the zoomed-in view; the dock
//! strip in the status corner is the zoomed-out view; both are chrome
//! around the same media element, so zooming never restarts, reloads
//! or re-buffers anything. Navigating away doesn't stop playback —
//! the host outlives every page.
//!
//! ## Zoom is visibility, not mounting
//!
//! The review screen stays mounted while a session exists and the dock
//! merely hides it (`display:none` keeps an `HTMLMediaElement`'s audio
//! running). Mount/unmount was the first design and it fails the whole
//! point: a fresh mount is a fresh element, and a fresh element starts
//! from a token re-resolve and a cold buffer.
//!
//! ## One audible source, enforced globally
//!
//! A capture-phase `play` listener on the document pauses every OTHER
//! media element the moment one starts, and tells the song engine
//! (`NowPlayingCtl`) to yield; the song engine starting pauses every
//! media element in return. Last starter wins, deterministically —
//! and the rule covers surfaces this module has never heard of (inline
//! mini players, note embeds), because it watches the document, not a
//! registry.

use dioxus::prelude::*;
use task_player_ui::{NowPlayingCtl, NpCmd};
use uuid::Uuid;

/// The host-owned media element id — the review screen mounts its
/// element under this id, and the dock strip drives the same element.
pub const HOST_MEDIA_ID: &str = "task-media-host-el";

/// What the shell is playing through the review surface.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewMedia {
    pub org: String,
    pub root_id: Uuid,
    pub path: String,
    /// Display title for the dock strip (the item's name, not the
    /// file's).
    pub title: String,
}

/// The unified session handle. Provided at the shell; any surface can
/// `open` media into it.
#[derive(Clone, Copy)]
pub struct MediaSession {
    pub current: Signal<Option<ReviewMedia>>,
    /// Zoomed in (full review screen) vs docked (strip).
    pub zoomed: Signal<bool>,
}

impl MediaSession {
    /// Open media in the zoomed view (docking is one click away).
    pub fn open(&self, media: ReviewMedia) {
        let (mut current, mut zoomed) = (self.current, self.zoomed);
        current.set(Some(media));
        zoomed.set(true);
    }

    pub fn zoom(&self) {
        let mut zoomed = self.zoomed;
        zoomed.set(true);
    }

    pub fn dock(&self) {
        let mut zoomed = self.zoomed;
        zoomed.set(false);
    }

    /// End the session: pause the element, drop the mount.
    pub fn close(&self) {
        let _ = dioxus::document::eval(&format!(
            "var v=document.getElementById('{HOST_MEDIA_ID}');if(v)v.pause();"
        ));
        let (mut current, mut zoomed) = (self.current, self.zoomed);
        current.set(None);
        zoomed.set(false);
    }
}

/// Install the session context. Once, at the shell, before the router.
pub fn provide_media_session() -> MediaSession {
    use_context_provider(|| MediaSession {
        current: Signal::new(None),
        zoomed: Signal::new(false),
    })
}

/// The host: the persistent review mount, the dock strip, and the
/// one-audible-source rule. Mounted once in the app shell.
#[component]
pub fn MediaHost() -> Element {
    let session = use_context::<MediaSession>();
    let ctl = use_context::<NowPlayingCtl>();
    let mut zoomed = session.zoomed;
    let current = session.current;

    // ── the one-source rule ─────────────────────────────────────
    // Media element starts → every other element pauses (JS side),
    // and the song engine yields (the message back to us).
    use_hook(|| {
        spawn(async move {
            let mut chan = dioxus::document::eval(
                "if(!window.__taskOnePlayer){window.__taskOnePlayer=true;\
                   document.addEventListener('play',function(e){\
                     var t=e.target;\
                     if(!t||!(t.tagName==='VIDEO'||t.tagName==='AUDIO'))return;\
                     document.querySelectorAll('video,audio').forEach(function(m){\
                       if(m!==t&&!m.paused){m.pause();}\
                     });\
                     dioxus.send(1);\
                   },true);\
                 }",
            );
            while chan.recv::<i32>().await.is_ok() {
                let mut cmd = ctl.cmd;
                let g = cmd.peek().0 + 1;
                cmd.set((g, NpCmd::Pause));
            }
        })
    });
    // Song engine starts → every media element yields. (The engine is
    // an AudioWorklet, not an element, so the document listener above
    // can't see it.)
    use_effect(move || {
        if (ctl.playing)() {
            let _ = dioxus::document::eval(
                "document.querySelectorAll('video,audio').forEach(function(m){\
                   if(!m.paused){m.pause();}\
                 });",
            );
        }
    });

    // ── dock transport state, polled off the host element ───────
    let mut el_playing = use_signal(|| false);
    let mut el_pos = use_signal(|| 0.0f64);
    let mut el_dur = use_signal(|| 0.0f64);
    use_hook(|| {
        spawn(async move {
            let mut chan = dioxus::document::eval(&format!(
                "setInterval(function(){{\
                   var v=document.getElementById('{HOST_MEDIA_ID}');\
                   if(v){{dioxus.send([v.paused?0:1,v.currentTime||0,v.duration||0]);}}\
                 }},400);"
            ));
            while let Ok(state) = chan.recv::<Vec<f64>>().await {
                if let [p, ct, d] = state[..] {
                    el_playing.set(p > 0.5);
                    el_pos.set(ct);
                    el_dur.set(d.max(0.0));
                }
            }
        })
    });

    let Some(media) = current() else {
        return rsx! {};
    };

    let frac = if el_dur() > 0.0 {
        (el_pos() / el_dur()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let time_label = format!(
        "{}:{:02} / {}:{:02}",
        (el_pos() as u32) / 60,
        (el_pos() as u32) % 60,
        (el_dur() as u32) / 60,
        (el_dur() as u32) % 60,
    );

    rsx! {
        // The ONE review mount — zoom toggles visibility around it.
        // Keyed by the media identity so switching files remounts, and
        // only that.
        div {
            key: "{media.root_id}-{media.path}",
            class: if zoomed() { "fixed inset-0 z-50" } else { "hidden" },
            files_ui::review::ReviewScreen {
                org: media.org.clone(),
                root_id: media.root_id,
                path: media.path.clone(),
                element_id: HOST_MEDIA_ID.to_string(),
                // Leaving the zoomed view docks it — playback carries
                // on in the strip. Ending playback is the strip's ✕.
                on_close: move |()| zoomed.set(false),
            }
        }
        if !zoomed() {
            div { class: "fixed bottom-8 right-4 z-40 flex w-72 flex-col overflow-hidden rounded-xl border border-border bg-card shadow-lg",
                div { class: "flex items-center gap-2 px-2.5 py-2",
                    button {
                        r#type: "button",
                        title: if el_playing() { "Pause" } else { "Play" },
                        class: "flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary hover:bg-primary hover:text-primary-foreground transition-colors",
                        onclick: move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            let _ = dioxus::document::eval(&format!(
                                "var v=document.getElementById('{HOST_MEDIA_ID}');\
                                 if(v){{if(v.paused){{v.play();}}else{{v.pause();}}}}"
                            ));
                        },
                        if el_playing() {
                            architect_ui::lucide_dioxus::Pause { size: 13 }
                        } else {
                            architect_ui::lucide_dioxus::Play { size: 13 }
                        }
                    }
                    // The title is the door back into the zoomed view.
                    button {
                        r#type: "button",
                        class: "min-w-0 flex-1 text-left",
                        onclick: move |_| zoomed.set(true),
                        span { class: "block truncate text-[13px] font-medium text-foreground hover:underline",
                            "{media.title}"
                        }
                        span { class: "block font-mono text-[10px] tabular-nums text-muted-foreground",
                            "{time_label}"
                        }
                    }
                    button {
                        r#type: "button",
                        title: "Open",
                        class: "flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground",
                        onclick: move |_| zoomed.set(true),
                        architect_ui::lucide_dioxus::Maximize2 { size: 12 }
                    }
                    button {
                        r#type: "button",
                        title: "Stop and close",
                        class: "flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-destructive",
                        onclick: move |_| session.close(),
                        architect_ui::lucide_dioxus::X { size: 12 }
                    }
                }
                // The strip's hairline progress — the zoomed timeline's
                // one-pixel echo.
                div { class: "h-0.5 w-full bg-muted",
                    div {
                        class: "h-full bg-primary",
                        style: "width: {frac * 100.0}%",
                    }
                }
            }
        }
    }
}
