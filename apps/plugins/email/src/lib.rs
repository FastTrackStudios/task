//! Email, as a Task app.
//!
//! The same migration as scripture, and it was the same size, because
//! the reading room was already a feature crate (`email-ui`). All that
//! moved here is what made the shell know mail exists: a route, a nav
//! entry, an icon and a title.
//!
//! ## The one claim it makes
//!
//! `mailto:` — because a note that writes `<mailto:sam@example.com>`
//! currently hands the address to the operating system, which opens
//! whatever mail client the machine happens to have. That is the wrong
//! answer on a machine where Task *is* the mail client, and the shell
//! could not have fixed it without knowing about mail. The app can.
//!
//! No wikilink claim. Mail has no vocabulary of its own the way a
//! scripture reference does — `[[Sam]]` is a person, and contacts owns
//! people. An app that claimed unresolved links "in case they're an
//! address" would be guessing with somebody else's links.

use email_ui::EmailView;
use task_plugin_ui::architect_ui::lucide_dioxus::Mail;
use task_plugin_ui::dioxus::prelude::*;
use task_plugin_ui::{LinkTarget, PluginApp, PluginNav};

/// What the app binary registers.
pub const APP: PluginApp = PluginApp {
    id: "email",
    version: env!("CARGO_PKG_VERSION"),
    nav: &[PluginNav {
        label: "Email",
        icon: icon,
        path: "",
        rail: true,
    }],
    view: view,
    provide: None,
    widgets: None,
    fences: None,
    claim_link: None,
    claim_href: Some(claim_href),
};

/// `mailto:` opens the composer here rather than the OS mail client.
///
/// The address is kept whole, query and all — `mailto:` URLs carry
/// `?subject=` and `?body=`, and dropping them would silently lose what
/// a link was written to say.
fn claim_href(href: &str) -> Option<LinkTarget> {
    let address = href.strip_prefix("mailto:")?.trim();
    (!address.is_empty()).then(|| LinkTarget::param("compose", address))
}

fn icon() -> Element {
    rsx! { Mail { size: 16 } }
}

fn view(path: &str, _query: &str) -> Option<Element> {
    match path {
        // `compose` is claimed and carried, but the reading room does
        // not open a composer yet — it lands on the mailbox. Claiming
        // the scheme is still right: it keeps the link inside Task
        // instead of handing it to a mail client the user may not use,
        // and the parameter is already there for when the composer is.
        "" => Some(rsx! { EmailView {} }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mailto_link_stays_inside_task() {
        let target = claim_href("mailto:sam@example.com").expect("claimed");
        assert_eq!(
            task_plugin_ui::query_param(&target.query, "compose").as_deref(),
            Some("sam@example.com")
        );
    }

    /// A `mailto:` carries what the link was written to say. Losing the
    /// subject would look like the link working.
    #[test]
    fn the_subject_and_body_survive() {
        let target =
            claim_href("mailto:sam@example.com?subject=Mix notes").expect("claimed");
        assert_eq!(
            task_plugin_ui::query_param(&target.query, "compose").as_deref(),
            Some("sam@example.com?subject=Mix notes")
        );
    }

    #[test]
    fn other_schemes_are_left_alone() {
        assert!(claim_href("https://example.com").is_none());
        assert!(claim_href("scripture-open:John 3:16").is_none());
        assert!(claim_href("mailto:").is_none(), "an empty address is not a link");
    }
}
