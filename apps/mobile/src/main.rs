use dioxus::prelude::*;
use ui::App;

mod watch_sync;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    // Error/crash telemetry — hold `_sentry` for the life of `main`
    // (`dioxus::launch` diverges, so binding it here is sufficient).
    // The tracing subscriber carries the Sentry layer so `warn!`/
    // `error!` events are captured; `.try_init()` (inside init_tracing)
    // makes a later dioxus subscriber-init a no-op rather than a panic.
    let _sentry = architect_telemetry::init_tracing("task-mobile", "info");

    // Apple Watch config bridge: activate the WCSession host and
    // register the sink `ui::watch_sync` publishes into (no-op off iOS).
    watch_sync::init();

    dioxus::launch(Root);
}

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
