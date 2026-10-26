use dioxus::prelude::*;
use ui::App;

mod ios_auth;
mod keyboard_bar;
mod live;
mod typing_keys;
mod watch_sync;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    // The release's version (crates/release-version), before anything reports it.
    task_release_version::set(task_release_version::of_this_build!());
    // Before anything can open a TLS socket: the graph carries both rustls
    // backends, so it cannot pick one itself, and a TLS dial without one
    // panics (on the main thread, where it aborts). Err = already set.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Error/crash telemetry — hold `_sentry` for the life of `main`
    // (`dioxus::launch` diverges, so binding it here is sufficient).
    // The tracing subscriber carries the Sentry layer so `warn!`/
    // `error!` events are captured; `.try_init()` (inside init_tracing)
    // makes a later dioxus subscriber-init a no-op rather than a panic.
    let (_sentry, otel) = architect_telemetry::init_tracing_full("task-mobile", "info");
    // LEAK the OTLP guard, deliberately. Its Drop shuts the exporters
    // down, and `init_tracing` (which is `init_tracing_full(..).0`)
    // dropped it on the spot — so with a collector configured, every
    // event after startup hit "BatchLogProcessor.Emit.AfterShutdown"
    // and nothing exported. This main hands control to the UI event
    // loop and never returns, so there is no shutdown point to flush
    // at anyway; the OS reclaims everything at exit. (The telemetry
    // crate's own docs prescribe exactly this for client apps.)
    std::mem::forget(otel);

    // Apple Watch config bridge: activate the WCSession host and
    // register the sink `ui::watch_sync` publishes into (no-op off iOS).
    watch_sync::init();
    // Central sign-in through the system browser sheet, so one sign-in
    // at the issuer covers every FastTrackStudio app on the device
    // (no-op off iOS — the login screen then offers the form only).
    ios_auth::init();
    // The app draws its own bar over the keyboard; drop the web view's.
    keyboard_bar::init();
    // The timer's Live Activity and the widgets' data (Swift half: build.rs).
    live::init();
    // A typing-test launch types through the web view's own keyboard input.
    typing_keys::init();

    // Before launch — the nav is built on first render, and an app
    // registered after that has no tab until something else redraws.
    task_plugins::register_all();

    // The phone is a node of its own (`offline.local`, `offline.peers`):
    // the backend in-process over the app container's data dir, booted
    // on first use. With no host configured the app runs on it; a host
    // URL (TASK_VOX_URL, a baked TASK_VOX_URL_NATIVE, or one chosen in
    // Settings) still wins. `TASK_NODE_DISABLE=1` keeps a pure client.
    if std::env::var("TASK_NODE_DISABLE").is_ok_and(|v| v != "0") {
        tracing::info!("node: disabled by TASK_NODE_DISABLE — remote only");
    } else if let Some(opts) = task_node_host::options_from_env() {
        task_node_host::install(opts);
    }

    // The webview's own colour, set before any HTML renders. Without it
    // a light-mode phone flashed white between the launch screen and the
    // first paint — the app is dark by default, and so is the launch
    // screen (LaunchBackground, apps/mobile/ios/app-plist.sh), so the
    // flash was the only light frame in the whole start.
    dioxus::LaunchBuilder::new()
        .with_cfg(
            dioxus::mobile::Config::new()
                .with_background_color(LAUNCH_BACKGROUND)
                // Links the app is opened with — the Action button, a
                // Shortcut, a widget (`app.fasttrackstudio.task://capture`
                // …): tao hands them over as `Opened`, cold launch included;
                // `ui::deep_link` goes where they say.
                .with_custom_event_handler(|event, _| {
                    if let dioxus::mobile::tao::event::Event::Opened { urls } = event {
                        for url in urls {
                            ui::deep_link::push(url.to_string());
                        }
                    }
                }),
        )
        .launch(Root);
}

/// The brand theme's dark background (`#09090b`), the colour the launch
/// screen and the loading screen sit on.
const LAUNCH_BACKGROUND: (u8, u8, u8, u8) = (0x09, 0x09, 0x0b, 0xff);

/// Make the webview behave like an app rather than a page.
///
/// The iOS shell is a WKWebView, so without this it keeps the browser
/// affordances a native app doesn't have: pinch-to-zoom, double-tap
/// zoom, and the focus-zoom that jumps the viewport when you tap a
/// field. All three read as "this is a website in a box".
///
/// - `maximum-scale=1, user-scalable=no` disables pinch and double-tap
///   zoom. WKWebView honours this; mobile *Safari* deliberately ignores
///   it for accessibility, which is why this belongs to the packaged app
///   and NOT to `ui`'s shared shell — the web build must stay zoomable.
/// - `viewport-fit=cover` lets the layout reach under the notch/home
///   indicator, matching the web build.
/// - `touch-action: manipulation` removes the ~300ms double-tap-to-zoom
///   delay the webview otherwise reserves on every tap, which is a large
///   part of why webview UIs feel laggy next to native ones.
const APP_VIEWPORT: &str = "width=device-width, initial-scale=1, \
maximum-scale=1, user-scalable=no, viewport-fit=cover";

/// Belt to the viewport's braces: `user-scalable=no` covers gesture
/// zoom, this covers the tap-delay and stops long-press selection
/// turning ordinary chrome into a text selection.
const APP_TOUCH_CSS: &str = "\
html{touch-action:manipulation}\
body{-webkit-user-select:none;user-select:none;-webkit-touch-callout:none}\
input,textarea,[contenteditable],[contenteditable] *{-webkit-user-select:text;user-select:text}";

#[component]
fn Root() -> Element {
    rsx! {
        document::Meta { name: "viewport", content: APP_VIEWPORT }
        document::Style { {APP_TOUCH_CSS} }
        document::Stylesheet { href: TAILWIND_CSS }
        App {}
    }
}
