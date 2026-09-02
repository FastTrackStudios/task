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

use finance_ui::{FinancesView, InvoicesView, LedgerView, offer_integrations, provide_stores};
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
    view,
    panel: None,
    claim_file: None,
    // The invoice store, and the billing offer other apps look up.
    provide: Some(provide),
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

fn view(path: &str, query: &str) -> Option<Element> {
    match path {
        "" => Some(rsx! { FinancesView {} }),
        // Work another app sent here to be billed arrives as ordinary
        // query parameters — the integration is a link, so the deep
        // link is all there is to receive.
        "invoices" => Some(rsx! { InvoicesView { sent: sent_work(query) } }),
        "ledger" => Some(rsx! { LedgerView {} }),
        _ => None,
    }
}

/// This app's store, and what it offers other apps.
///
/// The offer goes here rather than in the composition root because it
/// is this app's to make: what finance is willing to do for others is
/// finance's business, and the binary that assembles the build should
/// not have to know the list.
fn provide() {
    provide_stores();
    offer_integrations();
}

/// Work another app handed over, if this URL carries any.
///
/// `None` for an ordinary visit to the invoices screen, which is the
/// overwhelming case — the parameters only appear on a link built by
/// `finance_ui::offer_integrations`.
fn sent_work(query: &str) -> Option<finance_contract::Billable> {
    let what = task_plugin_ui::query_param(query, "bill")?;
    Some(finance_contract::Billable {
        what,
        client: task_plugin_ui::query_param(query, "client").unwrap_or_default(),
        minutes: task_plugin_ui::query_param(query, "minutes")
            .and_then(|m| m.parse().ok())
            .unwrap_or(0),
    })
}
