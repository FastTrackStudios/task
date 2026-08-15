//! The review surface (issue #270 + the freeframe-parity overhaul):
//! a full-screen review experience and a mini embeddable player over
//! the same core.
//!
//! - [`ReviewScreen`] — the full-viewport experience: top bar (version
//!   switcher, compare, share), video stage with frame-anchored
//!   annotations, a thin timeline with avatar-dot comment markers, a
//!   custom transport bar, and a 360px comment rail.
//! - [`MiniPlayer`] — the inline embed for file pages: stage + slim
//!   transport + marker timeline, with an expand button that opens the
//!   full screen as an overlay.
//!
//! Originals never stream — the player resolves *renditions* (issue
//! #269) over the `rendition` RPC and streams them from the ranged
//! rendition route, so seeking never downloads the whole file.
//!
//! ## Why the URL carries a token
//!
//! `<video src>` can't set an `Authorization` header and can't await an
//! RPC, so the player mints one signed media grant over vox — prefix
//! `files/renditions/{root_id}`, covering the file's whole rendition
//! ladder — and appends it as `?token=`. Minting goes through the
//! shared [`task_ui_core::media_grant`] cache; a failed mint yields an
//! empty suffix, which still plays while `TASK_ENFORCE_MEDIA_TOKEN` is
//! off (the same rollout contract as the stem player's grants).
//!
//! ## Why the player talks to the element through `document::eval`
//!
//! Dioxus media events don't expose `currentTime`/`buffered`/`paused`,
//! and transport ops are property writes on the live element. One
//! polling channel per mounted player carries the element's whole
//! transport state back into signals (see [`PlayerCtx`]); commands go
//! out as tiny one-shot evals addressed by element id.
//!
//! ## The visual system (ported from the freeframe reference, mapped
//! onto theme tokens)
//!
//! Surfaces and text ride the app theme (`bg-card`, `border-border`,
//! `text-muted-foreground`) so both themes hold. The identity lives in
//! three deliberate escapes: the indigo progress gradient, the
//! avatar-dot marker row under a 1px track, and the semantic color
//! language — amber = timecode attachment, purple = annotation,
//! sky/emerald = compare sides A/B. All numerals are mono + tabular.

use dioxus::prelude::*;
use files_proto::{
    AnnotationPoint, AnnotationStroke, FilesEvent, NewReviewComment, RenditionKind, Review,
    ReviewComment,
};
use uuid::Uuid;

mod mini;
mod panel;
mod progress;
mod screen;
mod stage;
mod transport;

pub use mini::MiniPlayer;
pub use screen::ReviewScreen;

/// What a share-guest boot needs to mount the screen (issue #272):
/// the review scoped to its link.
pub use files_proto::Review as ReviewInfo;

/// Extensions the review player mounts for. Matching is by name only —
/// the `rendition` RPC is the authority (a misnamed file simply
/// resolves no proxy and the player says so).
pub fn is_video_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or_default().to_lowercase();
    matches!(
        ext.as_str(),
        "mov" | "mp4" | "m4v" | "mkv" | "webm" | "avi" | "mxf" | "mts"
    )
}

/// The guest lane's "what can I see": its one review.
pub async fn guest_scoped_review(org: &str) -> Result<Review, String> {
    let reviews = crate::client(org)
        .await?
        .list_reviews(None)
        .await
        .map_err(|e| e.to_string())?;
    reviews
        .into_iter()
        .next()
        .ok_or_else(|| "this link has no review attached".to_string())
}

// ── streaming sources ─────────────────────────────────────────────

/// The streaming route's URL for a resolved rendition (`tok` is the
/// `?token=…` suffix, or empty).
fn rendition_url(
    org: &str,
    root_id: Uuid,
    kind: RenditionKind,
    file_id: &str,
    tok: &str,
) -> String {
    // Guests may be served from a different origin than the task server
    // (split-origin dev/static hosting) — pin media to the server there;
    // members stay origin-relative (the deployed same-origin shape).
    let base = task_ui_core::vox_session::guest_http_base().unwrap_or_default();
    format!(
        "{base}/org/{org}/files/renditions/{root_id}/{}/{file_id}{tok}",
        kind.tag()
    )
}

/// What the player streams: the proxy URL, and the filmstrip URL when
/// the source yields one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sources {
    pub proxy: String,
    pub filmstrip: Option<String>,
}

/// Resolve the opened file to its streamable sources: proxy + filmstrip
/// renditions over the RPC (generated on demand, cached server-side),
/// plus one grant covering both URLs. `at` pins a past version (the
/// switcher, issue #270 AC 4); `None` follows the checkpoint head.
pub(crate) async fn resolve_sources(
    org: &str,
    root_id: Uuid,
    path: &str,
    at: Option<String>,
) -> Result<Sources, String> {
    let c = crate::client(org).await?;
    let rendition = |kind: RenditionKind| {
        let c = c.clone();
        let at = at.clone();
        async move {
            match at {
                Some(commit) => c.rendition_at(root_id, path.to_owned(), commit, kind).await,
                None => c.rendition(root_id, path.to_owned(), kind).await,
            }
        }
    };
    let proxy = rendition(RenditionKind::Proxy720)
        .await
        .map_err(|e| e.to_string())?;
    // No filmstrip is not an error — the proxy still plays; the scrub
    // preview simply doesn't render.
    let filmstrip = rendition(RenditionKind::Filmstrip).await.ok();
    let tok =
        task_ui_core::media_grant::suffix_for_prefix(org, &format!("files/renditions/{root_id}"))
            .await;
    Ok(Sources {
        proxy: rendition_url(org, root_id, RenditionKind::Proxy720, &proxy.file_id, &tok),
        filmstrip: filmstrip
            .map(|f| rendition_url(org, root_id, RenditionKind::Filmstrip, &f.file_id, &tok)),
    })
}

// ── review RPC ────────────────────────────────────────────────────

/// The file's review if it exists — a pure read (follows renames
/// server-side). Browsing must never mint entities.
pub(crate) async fn find_review(
    org: &str,
    root_id: Uuid,
    path: &str,
) -> Result<Option<Review>, String> {
    crate::client(org)
        .await?
        .find_review(root_id, path.to_owned())
        .await
        .map_err(|e| e.to_string())
}

/// Get-or-create the file's review — called when feedback actually
/// starts (the first comment), never on mount.
pub(crate) async fn ensure_review(org: &str, root_id: Uuid, path: &str) -> Result<Review, String> {
    crate::client(org)
        .await?
        .review_for_file(root_id, path.to_owned())
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn fetch_comments(
    org: &str,
    review_id: Uuid,
) -> Result<Vec<ReviewComment>, String> {
    crate::client(org)
        .await?
        .review_comments(review_id)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn post_comment(
    org: &str,
    review_id: Uuid,
    comment: NewReviewComment,
) -> Result<ReviewComment, String> {
    crate::client(org)
        .await?
        .add_review_comment(review_id, comment)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn remove_comment(org: &str, id: Uuid) -> Result<(), String> {
    crate::client(org)
        .await?
        .delete_review_comment(id)
        .await
        .map_err(|e| e.to_string())
}

// ── timecode formats ──────────────────────────────────────────────

/// Parse a timecode — `ss(.f)`, `mm:ss`, or `h:mm:ss` — to seconds.
pub fn parse_timecode(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for part in parts {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 || !v.is_finite() {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// The shared playback-clock spelling (`m:ss` / `h:mm:ss`), saturated
/// from the element's float seconds.
pub(crate) fn display_timecode(secs: f64) -> String {
    // f64→u32 `as` saturates and maps NaN to 0.
    task_ui_core::format::duration_hms(secs as u32)
}

/// The proxy ladder's fixed frame rate. The transcoder does not carry
/// source fps through to the UI yet, so frame-addressed formats assume
/// the reference tool's default. FUTURE: plumb real fps from ffprobe
/// through the rendition RPC and drop this.
pub(crate) const ASSUMED_FPS: f64 = 24.0;

/// How the transport chip spells the clock — cycled by clicking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum TimeFormat {
    /// `m:ss / m:ss` — current and duration.
    #[default]
    Standard,
    /// SMPTE `HH:MM:SS:FF` at [`ASSUMED_FPS`] — current only.
    Smpte,
    /// Integer frame count at [`ASSUMED_FPS`] — `cur / dur`.
    Frames,
}

impl TimeFormat {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Standard => Self::Smpte,
            Self::Smpte => Self::Frames,
            Self::Frames => Self::Standard,
        }
    }

    /// The chip's text for `now` out of `duration`.
    pub(crate) fn render(self, now: f64, duration: f64) -> String {
        match self {
            Self::Standard => format!("{} / {}", display_timecode(now), display_timecode(duration)),
            Self::Smpte => {
                let now = if now.is_finite() { now.max(0.0) } else { 0.0 };
                let total_frames = (now * ASSUMED_FPS).floor();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let (t, f) = (now as u64, (total_frames % ASSUMED_FPS).max(0.0) as u64);
                format!(
                    "{:02}:{:02}:{:02}:{:02}",
                    t / 3600,
                    (t % 3600) / 60,
                    t % 60,
                    f
                )
            }
            Self::Frames => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let frames = |s: f64| {
                    if s.is_finite() {
                        (s.max(0.0) * ASSUMED_FPS).floor() as u64
                    } else {
                        0
                    }
                };
                format!("{} / {}", frames(now), frames(duration))
            }
        }
    }
}

// ── avatars ───────────────────────────────────────────────────
// The identity (hash, palette, initials) is the app-wide one from
// task-ui-core::avatar — the same person renders the same in the
// review rail and the org chrome.

pub(crate) use task_ui_core::avatar::{gradient_css as avatar_css, initials};

pub(crate) fn display_author(name: &str) -> String {
    if name.is_empty() {
        "Guest".to_string()
    } else {
        name.to_string()
    }
}

// ── unpinned comments ─────────────────────────────────────────

/// The wire type's `timecode_secs` is a bare f64 with no pinned flag;
/// a detached (general) comment stores this sentinel so it never
/// masquerades as frame-0 feedback: no marker dot, no chip, no seek.
pub(crate) const UNPINNED: f64 = -1.0;

pub(crate) fn is_pinned(timecode_secs: f64) -> bool {
    timecode_secs >= 0.0
}

// ── element interop ───────────────────────────────────────────────

/// Run a transport op on the live element. The snippet sees `v` (the
/// element), and runs only when it exists — a dropped player is a
/// no-op, never an error.
pub(crate) fn video_op(video_id: &str, op: &str) {
    let _ = dioxus::document::eval(&format!(
        "var v=document.getElementById('{video_id}');if(v){{{op}}}"
    ));
}

/// Seek the player to an absolute time.
pub(crate) fn seek_to(video_id: &str, secs: f64) {
    video_op(video_id, &format!("v.currentTime={secs};"));
}

pub(crate) fn seek_by(video_id: &str, delta: f64) {
    video_op(
        video_id,
        &format!("v.currentTime=Math.max(0,v.currentTime+({delta}));"),
    );
}

pub(crate) fn toggle_play(video_id: &str) {
    video_op(video_id, "if(v.paused){v.play();}else{v.pause();}");
}

pub(crate) fn pause(video_id: &str) {
    video_op(video_id, "v.pause();");
}

/// Fullscreen the whole player container (stage + bars stay visible),
/// or exit if it already is.
pub(crate) fn toggle_fullscreen(container_id: &str) {
    let _ = dioxus::document::eval(&format!(
        "var el=document.getElementById('{container_id}');\
         if(el){{if(document.fullscreenElement){{document.exitFullscreen();}}\
         else{{el.requestFullscreen();}}}}"
    ));
}

/// An element's CSS-pixel size, via `getBoundingClientRect` (an SVG
/// element's `clientWidth`/`Height` are 0 on Firefox).
pub(crate) async fn element_dims(id: &str) -> (f64, f64) {
    let mut e = dioxus::document::eval(&format!(
        "var el=document.getElementById('{id}');\
         if(el){{var r=el.getBoundingClientRect();dioxus.send([r.width,r.height]);}}\
         else{{dioxus.send([0,0]);}}"
    ));
    e.recv::<(f64, f64)>().await.unwrap_or((0.0, 0.0))
}

/// One mounted player's live transport state, fed by a 200ms polling
/// channel against the element and shared through context so the
/// stage, timeline, transport bar, and rail all read one clock.
///
/// Commands (seek, rate, mute…) are one-shot evals; the poll is the
/// single source of truth flowing back, so external changes (native
/// fullscreen controls, the element ending) stay honest.
#[derive(Clone, Copy)]
pub(crate) struct PlayerCtx {
    pub now: Signal<f64>,
    pub duration: Signal<f64>,
    pub buffered: Signal<f64>,
    pub paused: Signal<bool>,
    pub muted: Signal<bool>,
    pub looped: Signal<bool>,
    pub rate: Signal<f64>,
    /// The video's *used* box inside the letterboxed stage —
    /// `[left, top, width, height]` in CSS px relative to the stage.
    /// Annotations are authored and displayed inside this rect only,
    /// so letterbox bars are excluded and normalized coordinates stay
    /// meaningful (the reference tool's `VideoFrameConstraint`).
    pub frame_rect: Signal<(f64, f64, f64, f64)>,
}

impl PlayerCtx {
    pub(crate) fn use_ctx() -> Self {
        use_context::<Self>()
    }

    /// Create the signals and start the poll loop for `video_id`
    /// inside `stage_id`. `anchor_id` is the player's OUTERMOST
    /// container — the poll lives exactly as long as it does. The
    /// video element itself mounts late (after the rendition RPC
    /// resolves), so its absence is a skipped tick, never a shutdown.
    /// Call once per mounted player, then provide via context.
    pub(crate) fn install(video_id: &str, stage_id: &str, anchor_id: &str) -> Self {
        let ctx = Self {
            now: Signal::new(0.0),
            duration: Signal::new(0.0),
            buffered: Signal::new(0.0),
            paused: Signal::new(true),
            muted: Signal::new(false),
            looped: Signal::new(false),
            rate: Signal::new(1.0),
            frame_rect: Signal::new((0.0, 0.0, 0.0, 0.0)),
        };
        let mut c = ctx;
        let (video_id, stage_id, anchor_id) = (
            video_id.to_owned(),
            stage_id.to_owned(),
            anchor_id.to_owned(),
        );
        spawn(async move {
            // The interval self-terminates when the element leaves the
            // DOM, and the recv loop ends with the channel — unmount
            // cleans both sides up.
            let mut chan = dioxus::document::eval(&format!(
                "var t=setInterval(function(){{\
                   if(!document.getElementById('{anchor_id}')){{clearInterval(t);return;}}\
                   var v=document.getElementById('{video_id}');\
                   if(!v){{return;}}\
                   var b=0;try{{if(v.buffered.length)b=v.buffered.end(v.buffered.length-1);}}catch(e){{}}\
                   var s=document.getElementById('{stage_id}');\
                   var fx=0,fy=0,fw=0,fh=0;\
                   if(s&&v.videoWidth>0&&v.videoHeight>0){{\
                     var cw=s.clientWidth,ch=s.clientHeight;\
                     var sc=Math.min(cw/v.videoWidth,ch/v.videoHeight);\
                     fw=v.videoWidth*sc;fh=v.videoHeight*sc;\
                     fx=(cw-fw)/2;fy=(ch-fh)/2;\
                   }}\
                   dioxus.send([v.currentTime,isFinite(v.duration)?v.duration:0,b,\
                     v.paused?1:0,v.muted?1:0,v.playbackRate,fx,fy,fw,fh]);\
                 }},200);"
            ));
            while let Ok(state) = chan.recv::<[f64; 10]>().await {
                c.now.set(state[0]);
                c.duration.set(state[1]);
                c.buffered.set(state[2]);
                c.paused.set(state[3] > 0.5);
                c.muted.set(state[4] > 0.5);
                c.rate.set(state[5]);
                c.frame_rect.set((state[6], state[7], state[8], state[9]));
            }
        });
        ctx
    }
}

// ── annotation authoring state ────────────────────────────────────

/// The reference tool's four annotation colors.
pub(crate) const STROKE_COLORS: [&str; 4] = ["#FF3B30", "#AF52DE", "#FF9500", "#34C759"];
/// Stroke width as a fraction of the frame width (~4px on 1000px).
pub(crate) const STROKE_WIDTH: f32 = 0.004;

/// Drawing + focus state shared by the stage, transport, timeline and
/// rail. `pending` is the drawing being authored (attached to the next
/// posted comment); `viewing` is a saved comment's drawing shown over
/// the frame.
#[derive(Clone, Copy)]
pub(crate) struct DrawCtx {
    pub draw_mode: Signal<bool>,
    pub color: Signal<&'static str>,
    pub pending: Signal<Vec<AnnotationStroke>>,
    /// The clock when the pending drawing's FIRST stroke landed — the
    /// frame the drawing belongs to. A posted drawing pins there, not
    /// wherever the user scrubbed afterwards.
    pub pending_at: Signal<Option<f64>>,
    pub viewing: Signal<Vec<AnnotationStroke>>,
    /// The comment whose drawing / focus state is active — timeline
    /// markers and rail rows highlight it together.
    pub focused: Signal<Option<Uuid>>,
}

impl DrawCtx {
    pub(crate) fn use_ctx() -> Self {
        use_context::<Self>()
    }

    pub(crate) fn install() -> Self {
        Self {
            draw_mode: Signal::new(false),
            color: Signal::new(STROKE_COLORS[0]),
            pending: Signal::new(Vec::new()),
            pending_at: Signal::new(None),
            viewing: Signal::new(Vec::new()),
            focused: Signal::new(None),
        }
    }

    /// Leave draw mode, discarding the unposted drawing (the explicit
    /// cancel — posting clears through the composer instead).
    pub(crate) fn cancel_drawing(&mut self) {
        self.draw_mode.set(false);
        self.pending.set(Vec::new());
        self.pending_at.set(None);
    }

    /// Focus a comment: show its drawing and highlight it everywhere.
    /// Frame-anchored feedback demands the paused frame it was drawn
    /// on, so callers seek+pause alongside.
    pub(crate) fn focus(&mut self, comment: &ReviewComment) {
        self.focused.set(Some(comment.id));
        self.viewing.set(comment.annotation.clone());
    }

    pub(crate) fn clear_focus(&mut self) {
        self.focused.set(None);
        self.viewing.set(Vec::new());
    }
}

/// Normalize a pointer position against the frame rect (stage-relative
/// pointer, frame-relative output), clamped into the frame.
pub(crate) fn normalized_point(
    x: f64,
    y: f64,
    rect: (f64, f64, f64, f64),
) -> Option<AnnotationPoint> {
    let (fx, fy, fw, fh) = rect;
    if fw <= 0.0 || fh <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(AnnotationPoint {
        x: (((x - fx) / fw).clamp(0.0, 1.0)) as f32,
        y: (((y - fy) / fh).clamp(0.0, 1.0)) as f32,
    })
}

/// The review conversation behind one opened file — shared via
/// context so the timeline markers and the comment rail read one list.
#[derive(Clone, Copy)]
pub(crate) struct ReviewData {
    pub org: ReadSignal<String>,
    pub root_id: ReadSignal<Uuid>,
    pub path: ReadSignal<String>,
    /// The file's review, if it exists (a pure read — the entity is
    /// minted by the first posted comment, never by browsing).
    pub scope: Resource<Result<Option<Review>, String>>,
    pub review_id: Memo<Option<Uuid>>,
    pub comments: Resource<Option<Result<Vec<ReviewComment>, String>>>,
    /// Timecode-ascending (the reference default), stable on ties.
    pub sorted: Memo<Vec<ReviewComment>>,
}

/// Install [`ReviewData`] for this mount: resources, the sorted view,
/// and the live re-read on comment events. Provide via context.
pub(crate) fn use_review_data(
    org: ReadSignal<String>,
    root_id: ReadSignal<Uuid>,
    path: ReadSignal<String>,
) -> ReviewData {
    let scope = use_resource(move || {
        let (org, root_id, path) = (org(), root_id(), path());
        async move { find_review(&org, root_id, &path).await }
    });
    let review_id = use_memo(move || match &*scope.read() {
        Some(Ok(Some(review))) => Some(review.id),
        _ => None,
    });
    let comments = use_resource(move || {
        let org = org();
        let review_id = review_id();
        async move {
            match review_id {
                Some(id) => Some(fetch_comments(&org, id).await),
                None => None,
            }
        }
    });
    let sorted = use_memo(move || {
        let mut list = match &*comments.read() {
            Some(Some(Ok(list))) => list.clone(),
            _ => Vec::new(),
        };
        // Pinned feedback in timecode order; general (unpinned) notes
        // last — the reference tool's ordering.
        list.sort_by(|a, b| {
            let key = |c: &ReviewComment| {
                if is_pinned(c.timecode_secs) {
                    (0, c.timecode_secs)
                } else {
                    (1, 0.0)
                }
            };
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        list
    });

    // Live: a comment landing anywhere (another reviewer, another
    // device) re-reads this thread.
    crate::use_files_events(
        move || org(),
        move |event: FilesEvent| {
            let mut comments = comments;
            let touched = match &event {
                FilesEvent::ReviewCommentAdded(c) | FilesEvent::ReviewCommentDeleted(c) => {
                    Some(c.review_id)
                }
                _ => None,
            };
            if touched.is_some() && touched == *review_id.peek() {
                comments.restart();
            }
        },
    );

    ReviewData {
        org,
        root_id,
        path,
        scope,
        review_id,
        comments,
        sorted,
    }
}

/// A stroke's `<polyline points>` attribute in the normalized viewBox.
pub(crate) fn points_attr(stroke: &AnnotationStroke) -> String {
    stroke
        .points
        .iter()
        .map(|p| format!("{},{}", p.x, p.y))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_paths_are_recognized_by_extension() {
        assert!(is_video_path("cut.mov"));
        assert!(is_video_path("takes/Cut Final.MP4"));
        assert!(!is_video_path("mix.wav"));
        assert!(!is_video_path("notes.txt"));
        assert!(!is_video_path("no-extension"));
    }

    #[test]
    fn timecodes_parse_in_all_three_shapes() {
        assert_eq!(parse_timecode("90"), Some(90.0));
        assert_eq!(parse_timecode("1:30"), Some(90.0));
        assert_eq!(parse_timecode("1:00:05"), Some(3605.0));
        assert_eq!(parse_timecode("2.5"), Some(2.5));
        assert_eq!(parse_timecode(""), None);
        assert_eq!(parse_timecode("1:2:3:4"), None);
        assert_eq!(parse_timecode("abc"), None);
        assert_eq!(parse_timecode("-5"), None);
    }

    #[test]
    fn timecodes_render_readably() {
        assert_eq!(display_timecode(0.0), "0:00");
        assert_eq!(display_timecode(90.4), "1:30");
        assert_eq!(display_timecode(3605.0), "1:00:05");
        assert_eq!(display_timecode(f64::NAN), "0:00");
        assert_eq!(display_timecode(-3.0), "0:00");
    }

    #[test]
    fn time_formats_cycle_and_render() {
        assert_eq!(TimeFormat::Standard.next(), TimeFormat::Smpte);
        assert_eq!(TimeFormat::Frames.next(), TimeFormat::Standard);
        assert_eq!(TimeFormat::Standard.render(90.0, 200.0), "1:30 / 3:20");
        // 90.5s @ 24fps = frame 12 of second 90.
        assert_eq!(TimeFormat::Smpte.render(90.5, 200.0), "00:01:30:12");
        assert_eq!(TimeFormat::Frames.render(1.0, 2.0), "24 / 48");
        // Hostile clocks never panic the chip.
        assert_eq!(TimeFormat::Smpte.render(f64::NAN, 0.0), "00:00:00:00");
        assert_eq!(TimeFormat::Frames.render(f64::INFINITY, -1.0), "0 / 0");
    }

    #[test]
    fn authors_and_pins_read_correctly() {
        assert_eq!(display_author(""), "Guest");
        assert!(is_pinned(0.0));
        assert!(is_pinned(42.5));
        assert!(!is_pinned(UNPINNED));
    }

    #[test]
    fn pointer_positions_normalize_against_the_frame_rect() {
        // A 800x450 frame letterboxed 50px right/down in the stage.
        let rect = (50.0, 50.0, 800.0, 450.0);
        let p = normalized_point(450.0, 275.0, rect).expect("center");
        assert!((p.x - 0.5).abs() < 1e-6 && (p.y - 0.5).abs() < 1e-6);
        // Outside the frame clamps to the edge rather than escaping 0..=1.
        let p = normalized_point(0.0, 9000.0, rect).expect("clamped");
        assert_eq!((p.x, p.y), (0.0, 1.0));
        // An unmeasured frame captures nothing (no divide-by-zero).
        assert!(normalized_point(10.0, 10.0, (0.0, 0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn strokes_render_as_normalized_polylines() {
        let stroke = AnnotationStroke {
            points: vec![
                AnnotationPoint { x: 0.1, y: 0.2 },
                AnnotationPoint { x: 0.5, y: 0.75 },
            ],
            color: "#ff3355".into(),
            width: 0.004,
        };
        assert_eq!(points_attr(&stroke), "0.1,0.2 0.5,0.75");
    }

    #[test]
    fn rendition_urls_target_the_streaming_route() {
        let root = Uuid::nil();
        assert_eq!(
            rendition_url("acme", root, RenditionKind::Proxy720, "abc123", "?token=t"),
            format!("/org/acme/files/renditions/{root}/proxy-720/abc123?token=t")
        );
        assert_eq!(
            rendition_url("acme", root, RenditionKind::Filmstrip, "abc123", ""),
            format!("/org/acme/files/renditions/{root}/filmstrip/abc123")
        );
    }
}
