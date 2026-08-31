use dioxus::desktop::tao::window::WindowBuilder;
use dioxus::desktop::{Config, tao};
use dioxus::prelude::*;
use ui::App;

mod sync_service;
mod tray;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    // Error/crash telemetry — hold `_sentry` for the life of `main`
    // (`launch` diverges, so binding it here is sufficient). The tracing
    // subscriber carries the Sentry layer so `warn!`/`error!` events are
    // captured; `.try_init()` (inside init_tracing) makes a later dioxus
    // subscriber-init a no-op rather than a panic.
    let (_sentry, otel) = architect_telemetry::init_tracing_full("task-desktop", "info");
    // LEAK the OTLP guard, deliberately. Its Drop shuts the exporters
    // down, and `init_tracing` (which is `init_tracing_full(..).0`)
    // dropped it on the spot — so with a collector configured, every
    // event after startup hit "BatchLogProcessor.Emit.AfterShutdown"
    // and nothing exported. This main hands control to the UI event
    // loop and never returns, so there is no shutdown point to flush
    // at anyway; the OS reclaims everything at exit. (The telemetry
    // crate's own docs prescribe exactly this for client apps.)
    std::mem::forget(otel);

    // The sync agent is a separate process precisely so it outlives this
    // window; the app's part is making sure it exists. Off the main
    // thread — it runs a subprocess, and nothing on screen waits for it.
    std::thread::spawn(|| tracing::info!("{}", sync_service::ensure_installed()));

    // Register the apps this build ships. The composition root is the
    // one crate that may know both Task and a plugin; nothing lower
    // names either direction, which is what keeps the extension point
    // an extension point rather than more coupling.
    //
    // Before launch, because the nav is built on first render.
    task_plugin_ui::register(task_plugin_cooking::APP);

    let (pos, size, fullscreen) = window_placement();
    let mut window = WindowBuilder::new()
        .with_title("Task")
        // Frameless: the app draws its own title bar (drag surface +
        // window controls injected via `WindowChrome` below), the same
        // chrome the FastTrackStudio app runs — the native decorations
        // never match the app on Linux and waste a strip of dead space.
        .with_decorations(false)
        .with_min_inner_size(tao::dpi::LogicalSize::new(720.0, 480.0));
    // Explicit size ONLY when asked for: dioxus-desktop restores the
    // last debug session's position/size from its session cache, and it
    // (correctly) treats an explicit builder value as an override — so
    // an unconditional default here would pin every run to it and break
    // "reopens where I left it". Release builds have no session cache,
    // hence the sized fallback there.
    match size {
        Some((w, h)) => window = window.with_inner_size(tao::dpi::LogicalSize::new(w, h)),
        None if !cfg!(debug_assertions) => {
            window = window.with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0));
        }
        None => {}
    }
    // Position first: borderless fullscreen picks the monitor the
    // window is on, so placing it inside the target screen is what
    // selects that screen.
    if let Some((x, y)) = pos {
        window = window.with_position(tao::dpi::LogicalPosition::new(x, y));
    }
    if fullscreen {
        window = window.with_fullscreen(Some(tao::window::Fullscreen::Borderless(None)));
    }
    let cfg = Config::new().with_window(window).with_menu(None);
    LaunchBuilder::desktop().with_cfg(cfg).launch(Root);
}

/// Where the window opens, for multi-monitor desks. Placement is a
/// *runtime* concern, so it rides env vars — set them once (a `.env`
/// line, or the `dx serve` command) and every hot-reload and restart
/// lands the window in the same place instead of being dragged back.
/// Same knobs as the FastTrackStudio app:
///
/// - `TASK_WINDOW_POS="6560,0"` — top-left corner in desktop
///   coordinates (`kscreen-doctor -o` on KDE prints each screen's
///   geometry).
/// - `TASK_WINDOW_SIZE="2560x1440"` — inner size when not fullscreen.
/// - `TASK_WINDOW_FULLSCREEN=1` — borderless fullscreen on whichever
///   monitor the position lands on.
#[allow(clippy::type_complexity)]
fn window_placement() -> (Option<(f64, f64)>, Option<(f64, f64)>, bool) {
    fn pair(var: &str, sep: char) -> Option<(f64, f64)> {
        let raw = std::env::var(var).ok()?;
        let (a, b) = raw.split_once(sep)?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    }
    let fullscreen = std::env::var("TASK_WINDOW_FULLSCREEN")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    (
        pair("TASK_WINDOW_POS", ','),
        pair("TASK_WINDOW_SIZE", 'x'),
        fullscreen,
    )
}

// The three Task app mains (desktop / mobile / web) are deliberately
// thin: a stylesheet plus `ui::App`. Application behaviour belongs in
// the `ui` crate so every platform gets it. The ONE thing that lives
// here is window chrome — the drag/minimize/maximize/close callbacks
// and the edge-resize strips — because it is meaningless anywhere but a
// frameless desktop window, and `dioxus::desktop` does not exist on the
// other targets. The buttons themselves render inside the app's own top
// bar (`ui::chrome::WindowControls`), which reads the context provided
// here; the tab strip's slack is the drag surface.
#[component]
fn Root() -> Element {
    // Task in the taskbar, for the hours the window is closed and the
    // agent is still working.
    tray::use_tray();

    use_context_provider(|| task_ui_core::window_chrome::WindowChrome {
        drag: Callback::new(|()| dioxus::desktop::window().drag()),
        toggle_maximize: Callback::new(|()| dioxus::desktop::window().toggle_maximized()),
        minimize: Callback::new(|()| dioxus::desktop::window().set_minimized(true)),
        close: Callback::new(|()| dioxus::desktop::window().close()),
    });
    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        // The frameless WebView keeps the platform's default body margin
        // and white page background, which shows as a light border
        // around the app. Zero it; the app paints its own ground.
        document::Style {
            {"html,body{margin:0;padding:0;height:100%;overflow:hidden;}*{box-sizing:border-box;}"}
        }
        App {}
        ResizeHandles {}
    }
}

/// Invisible edge/corner strips that restore native-feeling resize on
/// the frameless window (decorations off also removes the compositor's
/// resize borders). Corners render after edges so they win the hit test.
#[component]
fn ResizeHandles() -> Element {
    use tao::window::ResizeDirection as Dir;
    let handles: &[(&str, Dir)] = &[
        ("top: 0; left: 12px; right: 12px; height: 5px; cursor: ns-resize;", Dir::North),
        ("bottom: 0; left: 12px; right: 12px; height: 5px; cursor: ns-resize;", Dir::South),
        ("left: 0; top: 12px; bottom: 12px; width: 5px; cursor: ew-resize;", Dir::West),
        ("right: 0; top: 12px; bottom: 12px; width: 5px; cursor: ew-resize;", Dir::East),
        ("top: 0; left: 0; width: 12px; height: 12px; cursor: nwse-resize;", Dir::NorthWest),
        ("top: 0; right: 0; width: 12px; height: 12px; cursor: nesw-resize;", Dir::NorthEast),
        ("bottom: 0; left: 0; width: 12px; height: 12px; cursor: nesw-resize;", Dir::SouthWest),
        ("bottom: 0; right: 0; width: 12px; height: 12px; cursor: nwse-resize;", Dir::SouthEast),
    ];
    rsx! {
        for (pos, dir) in handles.iter().copied() {
            div {
                style: "position: fixed; z-index: 2147483647; {pos}",
                onmousedown: move |_| {
                    let _ = dioxus::desktop::window().drag_resize_window(dir);
                },
            }
        }
    }
}
