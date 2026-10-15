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
use task_plugin_ui::architect_ui::lucide_dioxus::Wallet;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "finance",
    version: env!("CARGO_PKG_VERSION"),
    // One tab: the overview, invoices and ledger are a strip inside it
    // (`FinanceTabs`), not three entries in the sidebar.
    nav: &[PluginNav {
        label: "Finance",
        icon: icon_finances,
        path: "",
        // Money that has gone out and not come back is checked often.
        rail: true,
    }],
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

fn view(path: &str, query: &str) -> Option<Element> {
    // The screens are their own wasm chunk on the web, downloaded the
    // first time somebody opens this app; everything else the app
    // registers stays in the shell. A plain call everywhere else.
    task_plugin_ui::lazy_view!("finance", finance_screen, path, query)
}

fn finance_screen(path: &str, query: &str) -> Option<Element> {
    let screen = match path {
        "" => rsx! { FinancesView {} },
        // Work another app sent here to be billed arrives as ordinary
        // query parameters — the integration is a link, so the deep
        // link is all there is to receive.
        "invoices" => rsx! { InvoicesView { sent: sent_work(query) } },
        "ledger" => rsx! { LedgerView {} },
        _ => return None,
    };
    let here = path.to_owned();
    Some(rsx! {
        FinanceTabs { here }
        {screen}
    })
}

/// The app's three screens as one strip — the sidebar holds a single
/// Finance entry, and this is how you move within it.
#[component]
fn FinanceTabs(here: String) -> Element {
    const TABS: [(&str, &str); 3] = [
        ("", "Overview"),
        ("invoices", "Invoices"),
        ("ledger", "Ledger"),
    ];
    rsx! {
        nav { class: "mx-auto flex w-full max-w-5xl gap-1 px-4 pt-4 sm:px-8",
            "aria-label": "Finance",
            for (path, label) in TABS {
                Link {
                    key: "{label}",
                    to: task_plugin_ui::href("finance", path, ""),
                    class: if here == path {
                        "rounded-full bg-accent px-3.5 py-1.5 text-[14px] font-medium text-foreground"
                    } else {
                        "rounded-full px-3.5 py-1.5 text-[14px] text-muted-foreground hover:bg-accent/60 hover:text-foreground"
                    },
                    "{label}"
                }
            }
        }
    }
}

/// This app's store, and what it offers other apps.
///
/// The offer goes here rather than in the composition root because it
/// is this app's to make: what finance is willing to do for others is
/// finance's business, and the binary that assembles the build should
/// not have to know the list.
fn provide() {
    // Its own wasm chunk on the web: the invoice store's types and
    // fetchers download once, then install at the root.
    task_plugin_ui::lazy_provide!("finance_stores", provide_all)
}

fn provide_all() {
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
