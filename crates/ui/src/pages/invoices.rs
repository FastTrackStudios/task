//! `/invoices` — real, persisted invoicing over the finance `Invoicing`
//! service. Three things:
//!
//! 1. **Uninvoiced** — billable time not yet on any invoice, per project;
//!    generate a draft invoice from it.
//! 2. **Invoices** — the persisted list with Draft / Sent / Paid /
//!    Partially-paid status; mark sent, record payment, delete drafts.
//! 3. **Preview** — a printable document card for the selected invoice.
//!
//! State is the shared optimistic invoice store ([`crate::stores`]):
//! mutations patch the list instantly and reconcile against the server
//! (rollback + tray notification on failure) — no refresh counter. The
//! *derived* uninvoiced view can't be reconciled client-side, so the
//! mutations invalidate its reactivity key and it refetches.
//!
//! The bill-to is picked from the contacts directory (a person-chip),
//! so who you bill renders the same here as on `/contacts` and
//! `/members`.

use crate::format::money;
use std::collections::HashMap;

use chrono::Utc;
use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use contacts_proto::Contact;
use finance_proto::GenerateInvoice;
use finance_proto::invoice::{Invoice, InvoiceStatus};

use crate::orgs::{OrgMeta, OrgSelection};
use crate::pages::contacts::PersonChip;
use crate::stores;

/// `(variant, label)` for an invoice status badge.
fn status_badge(s: &InvoiceStatus) -> (StatusBadgeVariant, &'static str) {
    match s {
        InvoiceStatus::Draft => (StatusBadgeVariant::Neutral, "Draft"),
        InvoiceStatus::Sent => (StatusBadgeVariant::Warning, "Unpaid"),
        InvoiceStatus::Viewed => (StatusBadgeVariant::Warning, "Viewed"),
        InvoiceStatus::PartiallyPaid => (StatusBadgeVariant::Warning, "Partial"),
        InvoiceStatus::Paid => (StatusBadgeVariant::Success, "Paid"),
        InvoiceStatus::Overdue => (StatusBadgeVariant::Danger, "Overdue"),
        InvoiceStatus::Cancelled => (StatusBadgeVariant::Neutral, "Cancelled"),
        InvoiceStatus::Reversed => (StatusBadgeVariant::Neutral, "Reversed"),
    }
}

const FIELD: &str = "w-full rounded-lg border border-input bg-input/30 px-3 py-2 text-sm outline-none \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 placeholder:text-muted-foreground";

#[component]
pub fn InvoicesView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));
    let mut selected = use_signal(|| None::<Uuid>);

    // The shared optimistic store: one AtomResult for the invoice list.
    let invoices = stores::use_invoice_list();

    // Uninvoiced time is *derived* server-side (it reshapes whenever an
    // invoice is generated/deleted), so it can't reconcile from the
    // store — settled invoice mutations invalidate this reactivity key
    // and the resource refetches.
    let reactivity = architect::try_use_reactivity();
    let uninvoiced = use_resource(move || {
        if let Some(r) = reactivity {
            r.track(stores::UNINVOICED_KEY);
        }
        async move { crate::feeds::fetch_uninvoiced_multi(&slugs()).await }
    });
    let projects = use_resource(move || async move {
        crate::feeds::fetch_projects(&slugs())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect::<HashMap<Uuid, String>>()
    });
    // Contacts back the bill-to picker (active only).
    let contacts = use_resource(move || async move {
        crate::feeds::fetch_contacts(&slugs().into_iter().next().unwrap_or_default())
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c: &Contact| !c.archived)
            .collect::<Vec<Contact>>()
    });

    let store = stores::use_invoice_store();
    let un_rows = uninvoiced.read().clone().unwrap_or_default();
    let inv_rows: Vec<(architect::Id<Uuid>, stores::OrgInvoice)> =
        invoices.value().cloned().unwrap_or_default();
    let load_err = invoices.error().cloned();
    let first_load = invoices.is_waiting() && invoices.value().is_none();
    let proj_names = projects.read().clone().unwrap_or_default();
    let contact_rows = contacts.read().clone().unwrap_or_default();

    let selected_inv = selected().and_then(|id| {
        inv_rows
            .iter()
            .find(|(_, r)| r.invoice.id == id)
            .map(|(_, r)| r.invoice.clone())
    });

    // ── Billing summary — the money hero of the page ──────────────
    let mut outstanding = 0i64;
    let mut overdue = 0i64;
    let mut collected = 0i64;
    let mut draft_count = 0usize;
    let mut open_count = 0usize;
    for (_, row) in &inv_rows {
        let inv = &row.invoice;
        collected += inv.amount_paid_minor;
        match inv.status {
            InvoiceStatus::Draft => draft_count += 1,
            InvoiceStatus::Sent | InvoiceStatus::Viewed | InvoiceStatus::PartiallyPaid => {
                outstanding += inv.balance_minor;
                open_count += 1;
            }
            InvoiceStatus::Overdue => {
                outstanding += inv.balance_minor;
                overdue += inv.balance_minor;
                open_count += 1;
            }
            _ => {}
        }
    }
    let ready_total: i64 = un_rows.iter().map(|(_, g)| g.amount_minor).sum();
    let ready_count = un_rows.len();

    rsx! {
        div { class: "mx-auto flex w-full max-w-5xl flex-col gap-6 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Billing"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Invoices" }
                Text { variant: TextVariant::Muted,
                    "Everything you're owed, at a glance — generate from billable time, bill a contact, track what's paid."
                }
            }

            // ── Summary tiles: outstanding is the thesis ────────────
            div { class: "grid grid-cols-2 gap-3 sm:grid-cols-4",
                StatTile { label: "Outstanding", value: money(outstanding), accent: "primary".to_string(), hint: format!("{open_count} open") }
                StatTile { label: "Overdue", value: money(overdue), accent: "destructive".to_string(), hint: String::new() }
                StatTile { label: "Collected", value: money(collected), accent: "emerald".to_string(), hint: String::new() }
                StatTile { label: "Drafts", value: draft_count.to_string(), accent: "muted".to_string(), hint: String::new() }
            }

            // ── Uninvoiced time → generate ─────────────────────────
            if !un_rows.is_empty() {
                div { class: "flex flex-col gap-2",
                    div { class: "flex items-end justify-between gap-3",
                        Heading { level: HeadingLevel::H3, "Ready to invoice" }
                        span { class: "font-mono text-sm font-semibold tabular-nums text-primary",
                            "{money(ready_total)} · {ready_count} bucket" {if ready_count == 1 { "" } else { "s" }}
                        }
                    }
                    div { class: "flex flex-col gap-2",
                        for (slug , g) in un_rows {
                            UninvoicedRow {
                                key: "{slug}:{g.tag}:{g.project_id:?}",
                                slug: slug.clone(),
                                group: g.clone(),
                                label: g.project_id.and_then(|p| proj_names.get(&p).cloned()).unwrap_or_else(|| if g.tag.is_empty() { "General".into() } else { g.tag.clone() }),
                                contacts: contact_rows.clone(),
                            }
                        }
                    }
                }
            }

            // ── Persisted invoices ─────────────────────────────────
            div { class: "flex flex-col gap-2",
                Heading { level: HeadingLevel::H3, "All invoices" }
                if first_load {
                    crate::states::LoadingState {}
                } else if inv_rows.is_empty() {
                    if let Some(err) = load_err {
                        crate::states::ErrorState {
                            title: "Couldn't load invoices",
                            message: err,
                            on_retry: move |()| store.reload(),
                        }
                    } else {
                        EmptyState {
                            message: "No invoices yet — generate one from billable time above.".to_string(),
                        }
                    }
                } else {
                    Card { class: "overflow-hidden".to_string(),
                        TableContainer {
                            Table {
                                TableHeader {
                                    TableRow {
                                        TableHead { class: "text-[0.7rem] uppercase tracking-wider text-muted-foreground".to_string(), "Invoice" }
                                        TableHead { class: "text-right text-[0.7rem] uppercase tracking-wider text-muted-foreground".to_string(), "Total" }
                                        TableHead { class: "text-right".to_string(), "" }
                                    }
                                }
                                TableBody {
                                    for (id , row) in inv_rows {
                                        InvoiceRow {
                                            key: "{id}",
                                            pending: id.is_temp(),
                                            slug: row.slug.clone(),
                                            invoice: row.invoice.clone(),
                                            on_view: move |id| selected.set(Some(id)),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Printable preview of the selected invoice ──────────
            if let Some(inv) = selected_inv {
                InvoicePreview { invoice: inv }
            }
        }
    }
}

/// One summary tile in the billing header. `accent` tints the value +
/// its status dot: `primary` / `destructive` / `emerald` / `muted`.
#[component]
fn StatTile(label: String, value: String, hint: String, accent: String) -> Element {
    let (value_cls, dot_cls) = match accent.as_str() {
        "primary" => ("text-foreground", "bg-primary"),
        "destructive" => ("text-destructive", "bg-destructive"),
        "emerald" => ("text-emerald-500", "bg-emerald-500"),
        _ => ("text-muted-foreground", "bg-muted-foreground/60"),
    };
    rsx! {
        Card { class: "flex flex-col gap-1.5 p-3.5".to_string(),
            div { class: "flex items-center gap-1.5",
                span { class: "h-1.5 w-1.5 rounded-full {dot_cls}" }
                span { class: "text-[0.7rem] font-medium uppercase tracking-wider text-muted-foreground",
                    "{label}"
                }
            }
            span { class: "font-mono text-2xl font-semibold tabular-nums {value_cls}", "{value}" }
            if !hint.is_empty() {
                span { class: "text-xs text-muted-foreground", "{hint}" }
            }
        }
    }
}

#[component]
fn UninvoicedRow(
    slug: String,
    group: finance_proto::UninvoicedGroup,
    label: String,
    contacts: Vec<Contact>,
) -> Element {
    let muts = stores::use_invoice_mutations();
    let mut open = use_signal(|| false);
    let client = use_signal(|| label.clone());
    let mut net_days = use_signal(|| "30".to_string());

    let hrs = group.seconds as f64 / 3600.0;
    let project_id = group.project_id;
    let tag = group.tag.clone();
    let amount_minor = group.amount_minor;
    let currency = group.currency.clone();
    // Sub-label distinguishes a project bucket from a tag / general one.
    let kind = if group.project_id.is_some() {
        "project"
    } else if group.tag.is_empty() {
        "general"
    } else {
        "tag"
    };

    // Optimistic: a draft invoice (seeded with this group's totals)
    // appears in the list instantly and reconciles to the generated
    // one; the uninvoiced view refetches when the mutation settles.
    let generate = move |_| {
        if client.read().trim().is_empty() {
            return;
        }
        let req = GenerateInvoice {
            project_id,
            tag: tag.clone(),
            client_name: client.read().clone(),
            since: String::new(),
            until: String::new(),
            net_days: net_days.read().parse().unwrap_or(30),
        };
        muts.generate(slug.clone(), req, amount_minor, currency.clone());
        open.set(false);
    };

    rsx! {
        Card { class: "border-primary/30 bg-primary/5 p-3".to_string(),
            div { class: "flex flex-col gap-2",
                div { class: "flex items-center justify-between gap-3",
                    div { class: "flex min-w-0 flex-col",
                        div { class: "flex items-center gap-1.5",
                            span { class: "truncate text-sm font-medium text-foreground", "{label}" }
                            if kind != "project" {
                                StatusBadge { variant: StatusBadgeVariant::Neutral, label: kind.to_string(), class: "px-1.5 py-0 text-[10px]".to_string() }
                            }
                        }
                        span { class: "text-xs text-muted-foreground",
                            "{group.session_count} sessions · {hrs:.1}h · {money(group.amount_minor)}"
                        }
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| open.toggle(),
                        "Generate"
                    }
                }
                if open() {
                    div { class: "flex flex-col gap-3 border-t border-border/60 pt-3",
                        div { class: "flex flex-wrap items-start gap-3",
                            div { class: "flex flex-1 flex-col gap-1",
                                span { class: "text-xs text-muted-foreground", "Bill to" }
                                ContactPicker { contacts: contacts.clone(), value: client }
                            }
                            label { class: "flex w-20 flex-col gap-1 text-xs text-muted-foreground",
                                "Net days"
                                input { class: "{FIELD}", r#type: "number", value: "{net_days}", oninput: move |e| net_days.set(e.value()) }
                            }
                        }
                        div { class: "flex justify-end",
                            Button {
                                variant: ButtonVariant::Primary,
                                size: ButtonSize::Small,
                                on_click: generate,
                                "Create draft"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A searchable bill-to picker over the contacts directory. Free text is
/// allowed (defaults to the project label); picking a contact fills its
/// `full_name` and shows it as a person-chip.
#[component]
fn ContactPicker(contacts: Vec<Contact>, value: Signal<String>) -> Element {
    let mut value = value;
    let mut open = use_signal(|| false);

    let current = value();
    let q = current.trim().to_lowercase();
    // An exact-name match means the user has settled on a contact —
    // hide the suggestion list.
    let exact = contacts.iter().any(|c| c.full_name == current);
    let matches: Vec<Contact> = contacts
        .iter()
        .filter(|c| {
            let hay = format!("{} {}", c.full_name, c.emails).to_lowercase();
            q.is_empty() || hay.contains(&q)
        })
        .take(6)
        .cloned()
        .collect();

    let picked = contacts.iter().find(|c| c.full_name == current).cloned();

    rsx! {
        div { class: "relative flex flex-col gap-1.5",
            input {
                class: "{FIELD}",
                placeholder: "Search contacts or type a name…",
                value: "{value}",
                oninput: move |e| {
                    value.set(e.value());
                    open.set(true);
                },
                onfocusin: move |_| open.set(true),
            }
            if let Some(c) = picked {
                PersonChip {
                    name: c.full_name.clone(),
                    email: c.primary_email().unwrap_or_default().to_string(),
                    subtitle: c.organization.clone(),
                    size: 28,
                }
            }
            if open() && !exact && !matches.is_empty() {
                div { class: "absolute top-full z-30 mt-1 flex w-full flex-col overflow-hidden rounded-lg border border-border bg-popover shadow-lg",
                    for c in matches {
                        button {
                            key: "{c.id}",
                            r#type: "button",
                            class: "flex items-center px-2.5 py-2 text-left hover:bg-muted/60",
                            // mousedown fires before the input's blur, so the
                            // pick lands even as focus leaves the field.
                            onmousedown: {
                                let name = c.full_name.clone();
                                move |e: MouseEvent| {
                                    e.prevent_default();
                                    value.set(name.clone());
                                    open.set(false);
                                }
                            },
                            PersonChip {
                                name: c.full_name.clone(),
                                email: c.primary_email().unwrap_or_default().to_string(),
                                subtitle: c.organization.clone(),
                                size: 28,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn InvoiceRow(
    slug: String,
    invoice: Invoice,
    pending: bool,
    on_view: EventHandler<Uuid>,
) -> Element {
    let muts = stores::use_invoice_mutations();
    // Hold the slug in a Copy `Signal` so `act` stays `Copy` across the
    // multiple button closures below.
    let slug = use_signal(|| slug);
    let id = invoice.id;
    let (variant, badge) = status_badge(&invoice.status);
    let number = if invoice.number.is_empty() {
        "Draft".to_string()
    } else {
        invoice.number.clone()
    };
    let total = money(invoice.total_minor);
    let balance = invoice.balance_minor;
    let is_draft = invoice.status == InvoiceStatus::Draft;
    let is_open = matches!(
        invoice.status,
        InvoiceStatus::Sent
            | InvoiceStatus::Viewed
            | InvoiceStatus::PartiallyPaid
            | InvoiceStatus::Overdue
    );
    let date = invoice.issue_date.clone();

    // Optimistic: the row flips status / vanishes instantly, then
    // reconciles against the server (rollback + tray on failure).
    let act = move |kind: &'static str| match kind {
        "sent" => muts.mark_sent(slug(), id),
        "pay" => {
            let date = Utc::now().date_naive().to_string();
            muts.record_payment(slug(), id, balance, date);
        }
        "delete" => muts.delete(slug(), id),
        _ => {}
    };

    let row_cls = if pending { "opacity-60" } else { "" };

    rsx! {
        TableRow { class: row_cls.to_string(),
            TableCell {
                button {
                    r#type: "button",
                    class: "flex w-full items-center gap-2.5 text-left",
                    onclick: move |_| on_view.call(id),
                    StatusBadge { variant, label: badge.to_string(), class: "px-1.5 py-0 text-[10px]".to_string() }
                    div { class: "flex min-w-0 flex-col",
                        span { class: "truncate text-sm text-foreground", "{number}" }
                        span { class: "text-xs text-muted-foreground", "{date}" }
                    }
                }
            }
            TableCell { class: "text-right".to_string(),
                div { class: "flex flex-col items-end",
                    span { class: "font-mono text-sm tabular-nums text-foreground", "{total}" }
                    if balance > 0 {
                        span { class: "font-mono text-[11px] tabular-nums text-yellow-500", "{money(balance)} due" }
                    }
                }
            }
            TableCell { class: "text-right".to_string(),
                div { class: "flex items-center justify-end gap-1",
                    if is_draft {
                        Button { variant: ButtonVariant::Secondary, size: ButtonSize::Small, on_click: move |_| act("sent"), "Send" }
                        Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, on_click: move |_| act("delete"), "Delete" }
                    }
                    if is_open {
                        Button { variant: ButtonVariant::Primary, size: ButtonSize::Small, on_click: move |_| act("pay"), "Mark paid" }
                    }
                }
            }
        }
    }
}

#[component]
fn InvoicePreview(invoice: Invoice) -> Element {
    let number = if invoice.number.is_empty() {
        "DRAFT".to_string()
    } else {
        invoice.number.clone()
    };
    let (variant, badge) = status_badge(&invoice.status);
    rsx! {
        div { class: "flex flex-col gap-3",
            // The document card. Kept on a light surface so print output
            // is clean regardless of the app theme.
            div { id: "invoice-print",
                class: "flex flex-col gap-6 rounded-xl border border-border bg-white p-6 text-slate-900 shadow-sm sm:p-8",
                div { class: "flex items-start justify-between gap-4",
                    div { class: "flex flex-col gap-1",
                        span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.2em] text-slate-400", "Invoice" }
                        span { class: "font-mono text-lg font-semibold", "{number}" }
                    }
                    div { class: "flex flex-col items-end gap-2 text-right text-sm",
                        StatusBadge { variant, label: badge.to_string(), class: "px-2 py-0.5 text-[11px]".to_string() }
                        div { class: "text-slate-400", "Issued" }
                        div { class: "tabular-nums", "{invoice.issue_date}" }
                        if !invoice.due_date.is_empty() {
                            div { class: "text-slate-400", "Due {invoice.due_date}" }
                        }
                    }
                }
                table { class: "w-full border-collapse text-sm",
                    thead {
                        tr { class: "border-b border-slate-200 text-left text-[0.7rem] uppercase tracking-wider text-slate-400",
                            th { class: "py-2 font-medium", "Description" }
                            th { class: "py-2 text-right font-medium", "Hours" }
                            th { class: "py-2 text-right font-medium", "Amount" }
                        }
                    }
                    tbody {
                        for li in invoice.line_items.0.iter() {
                            tr { key: "{li.id}", class: "border-b border-slate-100",
                                td { class: "py-2", "{li.description}" }
                                td { class: "py-2 text-right tabular-nums", {format!("{:.2}", li.quantity_milli as f64 / 1000.0)} }
                                td { class: "py-2 text-right tabular-nums", "{money(li.line_total_minor)}" }
                            }
                        }
                    }
                    tfoot {
                        tr { class: "text-base font-semibold",
                            td { class: "py-3", "Total" }
                            td {}
                            td { class: "py-3 text-right tabular-nums", "{money(invoice.total_minor)}" }
                        }
                        if invoice.amount_paid_minor > 0 {
                            tr { class: "text-emerald-700",
                                td { class: "py-0.5", "Paid" }
                                td {}
                                td { class: "py-0.5 text-right tabular-nums", "{money(invoice.amount_paid_minor)}" }
                            }
                            tr { class: "font-semibold",
                                td { class: "py-0.5", "Balance" }
                                td {}
                                td { class: "py-0.5 text-right tabular-nums", "{money(invoice.balance_minor)}" }
                            }
                        }
                    }
                }
            }
            div { class: "flex justify-end",
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| {
                        let _ = dioxus::document::eval("window.print()");
                    },
                    "Print / Save PDF"
                }
            }
        }
    }
}
