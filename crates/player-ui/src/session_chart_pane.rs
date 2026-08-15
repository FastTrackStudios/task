//! Setlist chart pane — the active song's keyflow chart, engraved as a real
//! A4 page **document** and synced to the session playhead.
//!
//! Renders with the CPU engraver pipeline (`keyflow::engraver` layout →
//! fontless SVG string, wasm-safe — no canvas/wgpu), using the same
//! **Master Rhythm / paginated A4** layout the site's chart editor uses so the
//! measures fill each system to the page edge. All pages lay out side-by-side
//! in one row; the viewport fits the page **height** (so a page's full height
//! is always visible and a wide viewport shows more than one page), scrolls
//! horizontally, and supports drag-to-pan + wheel-to-zoom. Prev/Next just pans
//! to center the target page.
//!
//! - **chart source**: `SONG_CHARTS[project_guid].chart_text` (or the song's
//!   own `chart_text`), keyed off `ACTIVE_INDICES.song_index` +
//!   `SETLIST_STRUCTURE`.
//! - **static layer**: chart text → `keyflow::parse` → A4 paginated layout →
//!   one **fontless** SVG for the whole document (all pages in a row),
//!   re-generated only when the text changes. Rendered inline via
//!   `dangerous_inner_html` (NOT an `<img blob>`), with the engraving fonts
//!   injected once as `@font-face` (`editor_keyflow::font_face_css()`) so the
//!   SMuFL / chord / text glyphs resolve.
//! - **highlight overlay**: a second SVG with the document viewBox stacked on
//!   top; `ChartCursor` turns the playhead time into draw commands, so the
//!   active-measure highlight scales pixel-perfectly with the page under it.
//!   During playback the view auto-follows the cursor's page.
//!
//! Playhead model: `ACTIVE_INDICES.song_progress` (0..1 over the transport
//! timeline, whose 0 is the count-in start) maps onto the chart's own timeline,
//! whose 0 is the first real measure — the count-in is a header snippet with
//! negative-time positions. The transport's section starts already include the
//! count-in lead-in (`count_in_seconds`, or — for hydrated setlists that leave
//! it `None` — the first section's `start_seconds`), so we subtract it and the
//! seek lands on the right measure (`ChartCursor::compute_at_time`).

use dioxus::prelude::*;
use std::cell::RefCell;

use keyflow::engraver::export::{SvgExportConfig, SvgSerializer};
use keyflow::engraver::fonts::ChartFontBundle;
use keyflow::engraver::layout::ChartLayoutMode;
use keyflow::engraver::layout::chart::cursor::{
    ChartCursor, CursorConfig, CursorState, CursorStyle, HighlightCommand,
};
use keyflow::engraver::layout::chart::{ChartLayoutConfig, ChartLayoutEngine, ChartLayoutResult};
use keyflow::engraver::style::MStyle;
use session_ui::{ACTIVE_INDICES, SETLIST_STRUCTURE, SONG_CHARTS, SONG_VIEWS, SongView};

/// Zoom bounds for the pannable viewport (matches the site's editor).
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;
/// Fraction of the viewport height a fitted page fills (leaves a margin).
const FIT_MARGIN: f64 = 0.94;

// `SongView` + `SONG_VIEWS` now live in `session_ui` (beside SETLIST_STRUCTURE
// / SONG_CHARTS), so the navigator can show the effective key too — imported
// below.

// ─── Layout cache (one per pane — wasm is single-threaded) ────────────────

/// The engraved document: one wide SVG (all A4 pages in a row), its scene box,
/// and each page's horizontal placement (for centering / nav).
#[derive(Clone)]
struct DocRender {
    /// Whole-document fontless SVG (viewBox `0 0 total_w total_h`).
    svg: String,
    total_w: f64,
    total_h: f64,
    /// `(x, y, w, h)` scene box of each page — the white "paper" the pane paints
    /// behind the (transparent) document SVG, and the nav/centring targets.
    pages: Vec<(f64, f64, f64, f64)>,
}

struct Pane {
    engine: ChartLayoutEngine,
    /// (key, layout, rendered document)
    layout: Option<(u64, ChartLayoutResult, DocRender)>,
}

thread_local! {
    static PANE: RefCell<Option<Pane>> = const { RefCell::new(None) };
}

fn with_pane<R>(f: impl FnOnce(&mut Pane) -> R) -> Option<R> {
    PANE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let font_bundle = match ChartFontBundle::new() {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("chart pane: font bundle failed: {e}");
                    return None;
                }
            };
            let style: &'static MStyle = Box::leak(Box::new(MStyle::new()));
            let engine = font_bundle.create_layout_engine(style);
            *slot = Some(Pane {
                engine,
                layout: None,
            });
        }
        slot.as_mut().map(f)
    })
}

fn text_key(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Pane {
    /// Parse + lay out + serialize `text` under `view` if it isn't cached.
    fn ensure(&mut self, text: &str, view: &SongView) -> Option<DocRender> {
        let key = text_key(text) ^ view.cache_hash();
        if let Some((cached_key, _, doc)) = &self.layout {
            if *cached_key == key {
                return Some(doc.clone());
            }
        }

        let parsed = match keyflow::parse(text) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("chart pane: parse failed: {e}");
                return None;
            }
        };
        // Transpose / re-notate for display (no-op when the view is the song's
        // own key in letters with no capo). The source `text` is untouched.
        let chart = if view.is_identity() {
            parsed
        } else {
            keyflow::apply_view(&parsed, &view.to_chart_view())
        };

        // A4 page document with the **Master Rhythm** preset — the same layout
        // the site's chart editor uses: measures fill each system to the page
        // edge (not content-sized), true A4 proportions. `use_page_offsets`
        // lays the pages out side-by-side in scene space; we serialize the
        // whole scene as one SVG so the row can be panned/zoomed as a unit and
        // a single overlay (scene coords) lines up on any page. The paginated
        // path derives the count-in from the chart's `CountIn` section and gives
        // its beats negative-time positions (real measures at t=0), so the
        // playhead maps cleanly (see `ChartCursorOverlay`).
        let config = ChartLayoutConfig::master_rhythm().with_page_offsets(true);
        let mode = ChartLayoutMode::paginated_a4();
        let layout = self.engine.layout_chart_with_config(&chart, &mode, &config);

        let total_w = layout.total_width.max(1.0);
        let total_h = layout.total_height.max(60.0);
        // Transparent canvas: the pane paints each page's white paper itself, so
        // the gaps and margins between pages show the app background rather than
        // one big white sheet under everything.
        let mut cfg = SvgExportConfig::for_page(0.0, 0.0, total_w, total_h);
        cfg.background = None;
        let svg = SvgSerializer::new(cfg).serialize(&layout.scene);

        let pages: Vec<(f64, f64, f64, f64)> = if layout.pages.is_empty() {
            vec![(0.0, 0.0, total_w, total_h)]
        } else {
            layout
                .pages
                .iter()
                .map(|p| (p.x_offset, p.y_offset, p.width, p.height))
                .collect()
        };

        let doc = DocRender {
            svg,
            total_w,
            total_h,
            pages,
        };
        self.layout = Some((key, layout, doc.clone()));
        Some(doc)
    }

    /// Playhead time on the chart's own timeline → cursor state.
    fn cursor_state_at_time(&self, key: u64, chart_seconds: f64) -> Option<CursorState> {
        let (cached_key, layout, ..) = self.layout.as_ref()?;
        if *cached_key != key {
            return None;
        }
        let cursor = ChartCursor::new(CursorConfig {
            style: CursorStyle::MeasureHighlight,
            accent_color: [59, 130, 246, 255], // blue-500
            fill_alpha: 0.18,
            highlight_notehead: false,
            show_when_stopped: true,
            ..CursorConfig::default()
        });
        cursor.compute_at_time(layout, chart_seconds)
    }
}

/// `@font-face` CSS with the engraver's embedded fonts as data URIs — injected
/// once so the fontless chart SVGs resolve their families.
fn font_face_css() -> String {
    thread_local! {
        static CSS: RefCell<Option<String>> = const { RefCell::new(None) };
    }
    CSS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(css) = slot.as_ref() {
            return css.clone();
        }
        let css = editor_keyflow::font_face_css().unwrap_or_else(|e| {
            tracing::error!("chart pane: font-face css failed: {e}");
            String::new()
        });
        *slot = Some(css.clone());
        css
    })
}

/// Compute the transport time on the chart timeline for `progress`.
fn chart_seconds_for(progress: Option<f64>) -> Option<f64> {
    let p = progress?;
    let indices = ACTIVE_INDICES.read();
    let idx = indices.song_index?;
    drop(indices);
    let setlist = SETLIST_STRUCTURE.read();
    let song = setlist.songs.get(idx)?;
    let duration = song.duration();
    if duration <= 0.0 {
        return None;
    }
    // Count-in / lead-in before the first real measure. Prefer the explicit
    // `count_in_seconds`; hydrated setlists leave it `None`, where the first
    // section's `start_seconds` IS the lead-in (a 2-measure count is a ~3.78 s
    // gap @127 bpm). Section starts sit on measure boundaries and both values
    // are rounded seconds, so a seek lands exactly on a boundary where float
    // noise can drop it into the previous measure — bias forward ~15 ms.
    let count_in = song
        .count_in_seconds
        .or_else(|| song.sections.first().map(|s| s.start_seconds))
        .unwrap_or(0.0);
    const BOUNDARY_BIAS_S: f64 = 0.015;
    Some(p.clamp(0.0, 1.0) * duration - count_in + BOUNDARY_BIAS_S)
}

/// Inset (CSS px) of a focused page from the viewport edge it aligns to.
const PAGE_MARGIN_PX: f64 = 16.0;

/// pan_x for focusing page `i`: left-aligned (its left edge at `PAGE_MARGIN_PX`),
/// except the LAST page of a multi-page chart, which is right-aligned (its right
/// edge at the viewport's right minus the margin) so as much chart as possible
/// stays in view instead of trailing empty space.
fn page_pan_x(pages: &[(f64, f64, f64, f64)], i: usize, vw: f64, zoom: f64) -> Option<f64> {
    let (px, _py, pw, _ph) = pages.get(i).copied()?;
    if pages.len() > 1 && i + 1 == pages.len() {
        Some(vw - PAGE_MARGIN_PX - (px + pw) * zoom)
    } else {
        Some(PAGE_MARGIN_PX - px * zoom)
    }
}

// ─── Components ────────────────────────────────────────────────────────────

/// Chromatic key choices offered by the key selector (worship-friendly
/// spellings). `""`/`None` = render in the song's own key.
const KEY_CHOICES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// Mutate the active song's [`SongView`] (creating a default if absent).
fn update_song_view(guid: &str, f: impl FnOnce(&mut SongView)) {
    let mut views = SONG_VIEWS.write();
    f(views.entry(guid.to_string()).or_default());
}

/// The GUID of the currently-active setlist song, if any.
fn active_song_guid() -> Option<String> {
    let idx = ACTIVE_INDICES.read().song_index?;
    let setlist = SETLIST_STRUCTURE.read();
    setlist.songs.get(idx).map(|s| s.project_guid.clone())
}

/// Key / notation / capo selector for the active song. A display control: it
/// writes [`SONG_VIEWS`] (never the file), which re-engraves the chart. The
/// keyflow source stays as written. A dropdown for the key + a notation toggle
/// sit in the bar; capo + the "sounds like" caption live in the Advanced card.
#[component]
pub fn KeyBar() -> Element {
    let guid = use_memo(active_song_guid);
    let mut advanced = use_signal(|| false);

    let Some(guid) = guid() else {
        return rsx! {};
    };
    let view = SONG_VIEWS.read().get(&guid).cloned().unwrap_or_default();
    let cur_key = view.target_key.clone().unwrap_or_default();
    let notation = view.notation;
    let caption = view.to_chart_view().capo_caption();

    let sel_class = "rounded border border-border bg-card px-1.5 py-0.5 text-xs text-foreground";
    rsx! {
        div { class: "relative flex items-center gap-1.5",
            // Key dropdown (Original + 12 keys).
            select {
                class: "{sel_class}",
                onchange: {
                    let guid = guid.clone();
                    move |e: FormEvent| {
                        let v = e.value();
                        update_song_view(&guid, |sv| {
                            sv.target_key = (!v.is_empty()).then_some(v.clone());
                        });
                    }
                },
                option { value: "", selected: cur_key.is_empty(), "Key · Original" }
                for k in KEY_CHOICES {
                    option { value: k, selected: cur_key == k, "Key · {k}" }
                }
            }
            // Notation toggle: letters / Nashville numbers / Roman numerals.
            div { class: "flex overflow-hidden rounded border border-border",
                for (n , label , title) in [
                    (keyflow::NotationSystem::Letters, "A", "Letter names"),
                    (keyflow::NotationSystem::Nashville, "1", "Nashville numbers"),
                    (keyflow::NotationSystem::Roman, "I", "Roman numerals"),
                ] {
                    button {
                        key: "{label}",
                        title,
                        class: if notation == n {
                            "px-2 py-0.5 text-xs font-semibold bg-accent text-foreground"
                        } else {
                            "px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground"
                        },
                        onclick: {
                            let guid = guid.clone();
                            move |_| update_song_view(&guid, |sv| sv.notation = n)
                        },
                        "{label}"
                    }
                }
            }
            // Advanced (capo + caption + reset).
            button {
                class: if advanced() {
                    "rounded border border-border bg-accent px-2 py-0.5 text-xs text-foreground"
                } else {
                    "rounded border border-border px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground"
                },
                onclick: move |_| advanced.toggle(),
                "Advanced"
            }

            // Advanced card popup.
            if advanced() {
                div { class: "absolute right-0 top-8 z-20 w-64 rounded-lg border border-border bg-card p-3 shadow-xl",
                    div { class: "mb-2 flex items-center justify-between",
                        span { class: "text-xs font-semibold text-foreground", "Capo" }
                        select {
                            class: "{sel_class}",
                            onchange: {
                                let guid = guid.clone();
                                move |e: FormEvent| {
                                    let fret = e.value().parse::<u8>().unwrap_or(0);
                                    update_song_view(&guid, |sv| sv.capo = fret);
                                }
                            },
                            for fret in 0u8..=9 {
                                option {
                                    value: "{fret}",
                                    selected: view.capo == fret,
                                    if fret == 0 { "None" } else { "Fret {fret}" }
                                }
                            }
                        }
                    }
                    p { class: "mb-3 text-[11px] leading-snug text-muted-foreground",
                        if let Some(cap) = caption.clone() {
                            "{cap} — finger these shapes, capo on."
                        } else {
                            "Choose a key and capo; the chart re-renders in the shapes you finger. The song file is never changed."
                        }
                    }
                    button {
                        class: "w-full rounded-md bg-muted px-3 py-1.5 text-xs font-semibold text-muted-foreground hover:bg-accent hover:text-foreground",
                        onclick: {
                            let guid = guid.clone();
                            move |_| {
                                SONG_VIEWS.write().remove(&guid);
                                advanced.set(false);
                            }
                        },
                        "Reset to original"
                    }
                }
            }
        }
    }
}

/// The chart pane: active song's chart document + playhead highlight. Shows a
/// quiet placeholder when the song has no chart.
#[component]
pub fn SessionChartPane() -> Element {
    // (guid-independent) chart text for the ACTIVE song. Recomputes only when
    // the cursor's song, the setlist structure, or the hydrated chart changes.
    let chart_text = use_memo(move || {
        let indices = ACTIVE_INDICES.read();
        let idx = indices.song_index?;
        drop(indices);
        let setlist = SETLIST_STRUCTURE.read();
        let song = setlist.songs.get(idx)?;
        let charts = SONG_CHARTS.read();
        let text = charts
            .get(&song.project_guid)
            .map(|c| c.chart_text.clone())
            .or_else(|| song.chart_text.clone())?;
        // The active song's display view (transpose / notation / capo). Read
        // reactively so changing the key re-engraves the chart.
        let view = SONG_VIEWS
            .read()
            .get(&song.project_guid)
            .cloned()
            .unwrap_or_default();
        Some((text, view))
    });

    match chart_text() {
        Some((text, view)) => rsx! {
            ChartCanvas { text, view }
        },
        None => rsx! {
            div { style: "display:flex; align-items:center; justify-content:center; min-height:80px;",
                span { style: "font-size:12px; color:#52525b;", "No chart for this song." }
            }
        },
    }
}

/// Static page document + pan/zoom viewport + page nav. Re-renders only when the
/// chart text changes; the playhead lives in `ChartCursorOverlay`.
#[component]
fn ChartCanvas(text: String, view: SongView) -> Element {
    // Key the layout + overlay off both the source and the view, so switching
    // key / notation / capo re-engraves and re-anchors the playhead.
    let key = text_key(&text) ^ view.cache_hash();
    let doc = with_pane(|pane| pane.ensure(&text, &view)).flatten();

    let Some(doc) = doc else {
        return rsx! {
            div { style: "display:flex; align-items:center; justify-content:center; min-height:80px;",
                span { style: "font-size:12px; color:#ef4444;", "Chart failed to render." }
            }
        };
    };
    let n_pages = doc.pages.len().max(1);
    let total_w = doc.total_w;
    let total_h = doc.total_h;

    // Focused page (label + nav target); pan/zoom of the viewport; viewport size.
    let mut current = use_signal(|| 0usize);
    let mut zoom = use_signal(|| 1.0_f64);
    let mut pan_x = use_signal(|| 0.0_f64);
    let mut pan_y = use_signal(|| 0.0_f64);
    let mut dragging = use_signal(|| false);
    let mut last_mouse = use_signal(|| (0.0_f64, 0.0_f64));
    let mut viewport = use_signal(|| None::<(f64, f64)>);

    if current() >= n_pages {
        current.set(0);
    }

    // Initial fit: fit the page HEIGHT to the viewport (full height always
    // visible; a wide viewport then shows more than one page), center vertically
    // and focus the current page horizontally. Runs when the viewport is first
    // measured.
    {
        let pages = doc.pages.clone();
        let vp = viewport();
        use_effect(use_reactive!(|vp| {
            let Some((vw, vh)) = vp else { return };
            let z = (vh / total_h * FIT_MARGIN).clamp(ZOOM_MIN, ZOOM_MAX);
            zoom.set(z);
            pan_y.set((vh - total_h * z) / 2.0);
            let cur = *current.peek();
            if let Some(px) = page_pan_x(&pages, cur, vw, z) {
                pan_x.set(px);
            }
        }));
    }

    // Page focus: when the focused page changes (Prev/Next or playback follow),
    // pan horizontally to center it — keeping the current zoom, so the default
    // fit-height view just scrolls sideways to the page.
    {
        let pages = doc.pages.clone();
        let cur_val = current();
        use_effect(use_reactive!(|cur_val| {
            let Some((vw, _vh)) = *viewport.peek() else {
                return;
            };
            let z = *zoom.peek();
            if let Some(px) = page_pan_x(&pages, cur_val, vw, z) {
                pan_x.set(px);
            }
        }));
    }

    let transform = use_memo(move || {
        format!(
            "transform: translate({}px, {}px) scale({}); transform-origin: 0 0;",
            pan_x(),
            pan_y(),
            zoom()
        )
    });

    let cur = current().min(n_pages - 1);
    let at_first = cur == 0;
    let at_last = cur + 1 >= n_pages;
    let svg = doc.svg.clone();
    let page_boxes = doc.pages.clone();

    rsx! {
        document::Style { {font_face_css()} }
        div {
            style: "position:relative; width:100%; height:100%; min-height:0; overflow:hidden; background:var(--background,#0b0b0d); user-select:none; touch-action:none; cursor:{drag_cursor(dragging())};",

            onmounted: move |evt| {
                spawn(async move {
                    if let Ok(rect) = evt.data().get_client_rect().await {
                        viewport.set(Some((rect.size.width, rect.size.height)));
                    }
                });
            },
            onwheel: move |evt| {
                evt.prevent_default();
                let delta_y = evt.delta().strip_units().y;
                let old = zoom();
                let factor = if delta_y < 0.0 { 1.08 } else { 0.925 };
                let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                let c = evt.element_coordinates();
                let k = new / old;
                pan_x.set(c.x - (c.x - pan_x()) * k);
                pan_y.set(c.y - (c.y - pan_y()) * k);
                zoom.set(new);
            },
            onmousedown: move |evt| {
                dragging.set(true);
                let c = evt.client_coordinates();
                last_mouse.set((c.x, c.y));
            },
            onmousemove: move |evt| {
                if !dragging() { return; }
                let c = evt.client_coordinates();
                let (lx, ly) = last_mouse();
                pan_x.set(pan_x() + (c.x - lx));
                pan_y.set(pan_y() + (c.y - ly));
                last_mouse.set((c.x, c.y));
            },
            onmouseup: move |_| dragging.set(false),
            onmouseleave: move |_| dragging.set(false),

            // The transformed stage: white page "paper" + the (transparent)
            // document SVG + overlay, all in shared scene coordinates.
            div {
                style: "position:absolute; top:0; left:0; width:{total_w}px; height:{total_h}px; {transform}",
                for (i, (px, py, pw, ph)) in page_boxes.iter().enumerate() {
                    div {
                        key: "page-{i}",
                        style: "position:absolute; left:{px}px; top:{py}px; width:{pw}px; height:{ph}px; background:#ffffff; box-shadow:0 1px 10px rgba(0,0,0,0.35);",
                    }
                }
                div {
                    style: "position:absolute; inset:0;",
                    dangerous_inner_html: "{svg}",
                }
                ChartCursorOverlay {
                    layout_key: key,
                    view_w: total_w,
                    view_h: total_h,
                    current,
                }
            }

            // Page controls — only when the chart is more than one page.
            if n_pages > 1 {
                div {
                    style: "position:absolute; bottom:10px; left:50%; transform:translateX(-50%); display:flex; align-items:center; gap:8px; background:rgba(24,24,27,0.82); color:#fff; padding:5px 8px; border-radius:9px; font-size:12px; box-shadow:0 2px 10px rgba(0,0,0,0.25);",
                    onmousedown: move |evt| evt.stop_propagation(),
                    button {
                        style: "border:0; background:transparent; color:{nav_color(!at_first)}; cursor:{nav_cursor(!at_first)}; font-size:15px; padding:0 6px;",
                        disabled: at_first,
                        onclick: move |_| { if current() > 0 { current.set(current() - 1); } },
                        "‹ Prev"
                    }
                    span { style: "opacity:0.85; min-width:52px; text-align:center;", "Page {cur + 1} / {n_pages}" }
                    button {
                        style: "border:0; background:transparent; color:{nav_color(!at_last)}; cursor:{nav_cursor(!at_last)}; font-size:15px; padding:0 6px;",
                        disabled: at_last,
                        onclick: move |_| { if current() + 1 < n_pages { current.set(current() + 1); } },
                        "Next ›"
                    }
                }
            }
        }
    }
}

fn drag_cursor(dragging: bool) -> &'static str {
    if dragging { "grabbing" } else { "grab" }
}
fn nav_color(enabled: bool) -> &'static str {
    if enabled { "#ffffff" } else { "#71717a" }
}
fn nav_cursor(enabled: bool) -> &'static str {
    if enabled { "pointer" } else { "default" }
}

/// The playhead overlay over the whole document. Same viewBox as the document
/// SVG, absolutely positioned over it. Re-renders at cursor rate (only this
/// small component), and — while playing — advances `current` to follow the
/// cursor's page so the view scrolls to it.
#[component]
fn ChartCursorOverlay(
    layout_key: u64,
    view_w: f64,
    view_h: f64,
    current: Signal<usize>,
) -> Element {
    let (progress, playing) = {
        let indices = ACTIVE_INDICES.read();
        (indices.song_progress, indices.is_playing)
    };

    let chart_seconds = chart_seconds_for(progress);
    let state = chart_seconds
        .and_then(|t| with_pane(|pane| pane.cursor_state_at_time(layout_key, t)).flatten());

    // Auto-follow the cursor's page while playing (pages are 1-indexed).
    let cursor_page = state.as_ref().map(|s| s.page.saturating_sub(1) as usize);
    let mut current = current;
    use_effect(use_reactive!(|(cursor_page, playing)| {
        if playing
            && let Some(p) = cursor_page
            && p != *current.peek()
        {
            current.set(p);
        }
    }));

    let Some(state) = state else {
        return rsx! {};
    };

    rsx! {
        svg {
            view_box: "0 0 {view_w} {view_h}",
            preserve_aspect_ratio: "xMinYMin meet",
            style: "position:absolute; inset:0; width:100%; height:100%; pointer-events:none;",
            for (i, cmd) in state.commands.iter().enumerate() {
                {render_command(i, cmd)}
            }
            line {
                x1: "{state.cursor_x}",
                y1: "{state.cursor_y - 4.0}",
                x2: "{state.cursor_x}",
                y2: "{state.cursor_y + state.cursor_height + 4.0}",
                stroke: "rgba(59,130,246,0.9)",
                stroke_width: "1.5",
            }
        }
    }
}

fn rgba_css(c: &[u8; 4], alpha_mul: f32) -> String {
    let a = (c[3] as f32 / 255.0 * alpha_mul).clamp(0.0, 1.0);
    format!("rgba({},{},{},{a:.3})", c[0], c[1], c[2])
}

/// One cursor draw command → overlay SVG element. Glyph commands (notehead
/// glow) are skipped — the pane uses measure highlighting only.
fn render_command(i: usize, cmd: &HighlightCommand) -> Element {
    match cmd {
        HighlightCommand::FillRect {
            x,
            y,
            width,
            height,
            color,
        } => rsx! {
            rect {
                key: "{i}",
                x: "{x}",
                y: "{y}",
                width: "{width}",
                height: "{height}",
                fill: rgba_css(color, 1.0),
            }
        },
        HighlightCommand::FillRoundedRect {
            x,
            y,
            width,
            height,
            radius,
            color,
        } => rsx! {
            rect {
                key: "{i}",
                x: "{x}",
                y: "{y}",
                width: "{width}",
                height: "{height}",
                rx: "{radius}",
                fill: rgba_css(color, 1.0),
            }
        },
        HighlightCommand::StrokeLine {
            x,
            y_top,
            y_bottom,
            color,
            width,
        } => rsx! {
            line {
                key: "{i}",
                x1: "{x}",
                y1: "{y_top}",
                x2: "{x}",
                y2: "{y_bottom}",
                stroke: rgba_css(color, 1.0),
                stroke_width: "{width}",
            }
        },
        HighlightCommand::StrokeGlyph { .. } | HighlightCommand::FillGlyph { .. } => rsx! {},
    }
}
