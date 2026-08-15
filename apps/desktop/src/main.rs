use dioxus::desktop::{Config, tao::window::WindowBuilder};
use dioxus::prelude::*;
use ui::App;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    // Error/crash telemetry — hold `_sentry` for the life of `main`
    // (`launch` diverges, so binding it here is sufficient). The tracing
    // subscriber carries the Sentry layer so `warn!`/`error!` events are
    // captured; `.try_init()` (inside init_tracing) makes a later dioxus
    // subscriber-init a no-op rather than a panic.
    let _sentry = architect_telemetry::init_tracing("task-desktop", "info");

    let cfg = Config::new()
        .with_window(
            WindowBuilder::new()
                .with_title("Task")
                .with_inner_size(dioxus::desktop::tao::dpi::LogicalSize::new(1280.0, 800.0)),
        )
        .with_menu(None);
    LaunchBuilder::desktop().with_cfg(cfg).launch(Root);
}

// The three Task app mains (desktop / mobile / web) are deliberately
// thin: a stylesheet plus `ui::App`. Application behaviour belongs in
// the `ui` crate so every platform gets it. This file used to carry ~90
// lines of injected JS that no other platform had; all of it was either
// dead or superseded by Rust in `ui` — see the commit that removed it
// before adding anything like it back here.
#[component]
fn Root() -> Element {
    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        App {}
    }
}
