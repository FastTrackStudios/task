//! Finance, as a Task app.
//!
//! Three screens over one domain — the org's position, its invoices,
//! its ledger — and the app with the most to say about the shape of
//! the seam, because it is the first one that reads data it does not
//! own. An invoice is billed to a *contact*, against a *project*, for
//! *timer sessions*, and all three of those are core.
//!
//! It reaches them the way anything outside Task would: by calling the
//! services. `finance-ui` declares those calls itself against Task's
//! own clients. That is the arrangement the whole plugin design is
//! for — core is a service an app calls, not a crate the app has to be
//! inside of — and it is the same arrangement that lets something that
//! is not a Task front end at all, a website pulling its setlists,
//! read the same data the same way.

use finance_ui::{FinancesView, InvoicesView, LedgerView, provide_stores};
use task_plugin_ui::architect_ui::lucide_dioxus::{ReceiptText, Scale, Wallet};
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "finance",
    version: env!("CARGO_PKG_VERSION"),
    nav: &[
        PluginNav {
            label: "Finances",
            icon: icon_finances,
            path: "",
            rail: false,
        },
        PluginNav {
            label: "Invoices",
            icon: icon_invoices,
            path: "invoices",
            // The one screen with a recurring errand attached — money
            // that has gone out and not come back is checked often.
            rail: true,
        },
        PluginNav {
            label: "Ledger",
            icon: icon_ledger,
            path: "ledger",
            rail: false,
        },
    ],
    view: view,
    panel: None,
    claim_file: None,
    provide: Some(provide_stores),
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: None,
};

fn icon_finances() -> Element {
    rsx! { Wallet { size: 16 } }
}

fn icon_invoices() -> Element {
    rsx! { ReceiptText { size: 16 } }
}

fn icon_ledger() -> Element {
    rsx! { Scale { size: 16 } }
}

fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { FinancesView {} }),
        "invoices" => Some(rsx! { InvoicesView {} }),
        "ledger" => Some(rsx! { LedgerView {} }),
        _ => None,
    }
}
