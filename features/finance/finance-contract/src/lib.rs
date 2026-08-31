//! What the finance app offers other apps.
//!
//! This crate is a **contract**: types, no behaviour, no dependencies.
//! Both ends of an integration depend on it and neither depends on the
//! other, which is what lets each one build and run without the other
//! being present.
//!
//! It is also the key. `task_plugin_ui::offered::<Billing>(..)` looks
//! up by [`std::any::TypeId`], so the type *is* the name — there is no
//! string to spell wrong on one side, and changing the shape of a
//! contract is a compile error in both apps rather than a lookup that
//! silently stops matching.
//!
//! ```ignore
//! // finance, in its `provide`:
//! task_plugin_ui::offer("finance", Billing { bill_href });
//!
//! // any other app:
//! if let Some(b) = task_plugin_ui::offered::<Billing>(|id| enabled.contains(id)) {
//!     // render an "Invoice…" action pointing at (b.bill_href)(&work)
//! }
//! ```

/// A piece of work somebody could be billed for.
///
/// Deliberately thin, and deliberately not finance's own invoice
/// model: the offering app should not force its internal shape on
/// everyone who wants to hand it something. This is what any app can
/// say about work without knowing what an invoice is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Billable {
    /// What it was, in a person's words — "30-minute consult".
    pub what: String,
    /// Who it was for, if the asking app knows. A display name, not an
    /// id: the two apps need not agree on a contact registry to agree
    /// on a person's name.
    pub client: String,
    /// How long it took. Zero when the work is not time-shaped.
    pub minutes: u32,
}

/// Finance's offer: a way to bill something.
///
/// A URL rather than a call, on purpose. Raising an invoice is a
/// decision with a form attached — who exactly, what rate, what terms —
/// so the integration hands somebody to the screen that asks, prefilled.
/// An app that could silently create invoices in another app's ledger
/// would be a worse thing to have built.
#[derive(Clone, Copy)]
pub struct Billing {
    /// Where to go to bill this work. The result is a Task URL, ready
    /// for a `Link`.
    pub bill_href: fn(&Billable) -> String,
}
