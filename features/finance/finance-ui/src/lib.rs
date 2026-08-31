//! Finance — invoicing, the ledger, and the money screens.
//!
//! Three views of one domain: **Finances** (the org's position),
//! **Invoices** (bill uninvoiced time, record payments) and **Ledger**
//! (accounts and their transactions). They share a service surface and
//! an optimistic store, so they share a crate.
//!
//! ## Reading core data
//!
//! These screens also read things finance does not own — the contacts
//! an invoice is billed to, the projects work is booked against, the
//! timer sessions that become billable hours. Those calls are declared
//! here, against Task's own service clients, rather than reached for
//! through the shell. That is the arrangement an app is supposed to
//! have with the platform: core is a service you call, not a crate you
//! are inside of.
//!
//! Mounted by `task-plugin-finance`.

use std::collections::HashMap;

use architect::{AtomResult, Id, Mutation, StoreEntity, use_mutation};
use architect_ui::prelude::*;
use chrono::{Datelike, Utc};
use dioxus::prelude::*;
use task_stores::{run_create, use_multi_org_list};
use task_ui_core::feeds;
use task_ui_core::format::money;
use task_ui_core::orgs::{OrgMeta, OrgSelection};
use uuid::Uuid;

use contacts_proto::Contact;
use finance_proto::AccountBalance;
use finance_proto::GenerateInvoice;
use finance_proto::invoice::{Invoice, InvoiceStatus};
use finance_proto::ledger::{Account, AccountKind, Transaction};
use task_ui_core::avatar::PersonChip;

// ─────────────────────────────────────────────────────────────────────
// Core reads — Task's own services, called as services
// ─────────────────────────────────────────────────────────────────────

feeds! {
    contacts_proto::ContactsClient {
        /// The org's contacts — who an invoice is billed to.
        fetch_contacts() -> Vec<contacts_proto::Contact>
            = list_contacts() as "list contacts";
    }
}

// ── finance / invoicing ─────────────────────────────────────────────

async fn invoicing(slug: &str) -> Result<finance_proto::InvoicingClient, String> {
    task_ui_core::vox_clients::establish_for::<finance_proto::InvoicingClient>(slug).await
}

/// All invoices in an org, newest first.
pub async fn fetch_invoices(slug: &str) -> Result<Vec<finance_proto::Invoice>, String> {
    invoicing(slug)
        .await?
        .list_invoices()
        .await
        .map_err(|e| format!("{slug}: list invoices: {e:?}"))
}

/// Per-project billable time not yet invoiced, in an org.
pub async fn fetch_uninvoiced(slug: &str) -> Result<Vec<finance_proto::UninvoicedGroup>, String> {
    invoicing(slug)
        .await?
        .uninvoiced()
        .await
        .map_err(|e| format!("{slug}: uninvoiced: {e:?}"))
}

/// Generate + persist a draft invoice from a project's billable time.
pub async fn generate_invoice(
    slug: &str,
    req: finance_proto::GenerateInvoice,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .generate_invoice(req)
        .await
        .map_err(|e| format!("{slug}: generate invoice: {e:?}"))
}

/// Issue an invoice (assign number, lock).
pub async fn invoice_mark_sent(
    slug: &str,
    id: uuid::Uuid,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .mark_sent(id)
        .await
        .map_err(|e| format!("{slug}: mark sent: {e:?}"))
}

/// Record a payment against an invoice.
pub async fn invoice_record_payment(
    slug: &str,
    id: uuid::Uuid,
    amount_minor: i64,
    date: String,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .record_invoice_payment(id, amount_minor, date)
        .await
        .map_err(|e| format!("{slug}: record payment: {e:?}"))
}

/// Delete a draft invoice (un-bills its sessions).
pub async fn invoice_delete(slug: &str, id: uuid::Uuid) -> Result<(), String> {
    invoicing(slug)
        .await?
        .delete_invoice(id)
        .await
        .map_err(|e| format!("{slug}: delete invoice: {e:?}"))
}

// ── finance / ledger ────────────────────────────────────────────────

async fn ledger(slug: &str) -> Result<finance_proto::LedgerClient, String> {
    task_ui_core::vox_clients::establish_for::<finance_proto::LedgerClient>(slug).await
}

/// Resolve the org's (single) finance book id, if one exists yet.
async fn ledger_book_id(
    client: &finance_proto::LedgerClient,
    slug: &str,
) -> Result<Option<uuid::Uuid>, String> {
    let books = client
        .books()
        .await
        .map_err(|e| format!("{slug}: books: {e:?}"))?;
    Ok(books.first().map(|b| b.id))
}

/// Every account in an org's (single) book, paired with its current
/// balance. Returns `(account, balance)` rows. Empty when the org has
/// no book / accounts yet.
pub async fn fetch_ledger_accounts(
    slug: &str,
) -> Result<Vec<(finance_proto::Account, finance_proto::AccountBalance)>, String> {
    let client = ledger(slug).await?;
    let Some(book_id) = ledger_book_id(&client, slug).await? else {
        return Ok(Vec::new());
    };
    let accounts = client
        .accounts(book_id)
        .await
        .map_err(|e| format!("{slug}: accounts: {e:?}"))?;
    let balances = client
        .balances(book_id, None)
        .await
        .map_err(|e| format!("{slug}: balances: {e:?}"))?;
    let out = accounts
        .into_iter()
        .map(|a| {
            let bal = balances
                .iter()
                .find(|b| b.account_id == a.id)
                .cloned()
                .unwrap_or_else(|| finance_proto::AccountBalance {
                    account_id: a.id,
                    balance_minor: a.opening_balance_minor,
                    currency: a.currency.clone(),
                });
            (a, bal)
        })
        .collect();
    Ok(out)
}

/// Recent ledger transactions across every account in an org's book,
/// newest first. Pulls each account's history and de-dupes by
/// transaction id (a double-entry txn touches ≥2 accounts).
pub async fn fetch_ledger_transactions(
    slug: &str,
) -> Result<Vec<finance_proto::Transaction>, String> {
    let client = ledger(slug).await?;
    let Some(book_id) = ledger_book_id(&client, slug).await? else {
        return Ok(Vec::new());
    };
    let accounts = client
        .accounts(book_id)
        .await
        .map_err(|e| format!("{slug}: accounts: {e:?}"))?;
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<finance_proto::Transaction> = Vec::new();
    for a in accounts {
        let txns = client
            .account_transactions(a.id, None, None, 100)
            .await
            .map_err(|e| format!("{slug}: account transactions: {e:?}"))?;
        for t in txns {
            if seen.insert(t.id) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

/// Invoices across several orgs, slug-tagged, newest first.
pub async fn fetch_invoices_multi(slugs: &[String]) -> Vec<(String, finance_proto::Invoice)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(rows) = fetch_invoices(slug).await {
            out.extend(rows.into_iter().map(|i| (slug.clone(), i)));
        }
    }
    out.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    out
}

/// Uninvoiced groups across several orgs, slug-tagged.
pub async fn fetch_uninvoiced_multi(
    slugs: &[String],
) -> Vec<(String, finance_proto::UninvoicedGroup)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(rows) = fetch_uninvoiced(slug).await {
            out.extend(rows.into_iter().map(|g| (slug.clone(), g)));
        }
    }
    out
}

/// Active projects across the selected orgs — what work is booked
/// against.
pub async fn fetch_projects(slugs: &[String]) -> Result<Vec<project_proto::ProjectInfo>, String> {
    task_ui_core::feeds::fan_out(
        slugs,
        "list",
        |c: project_proto::ProjectServiceClient| async move { c.list().await },
    )
    .await
}

/// Every session in one org (all members), newest first — the operator
/// sees contractors' logged time, not only their own.
pub async fn fetch_org_sessions(slug: &str) -> Result<Vec<timer_proto::WorkSession>, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<timer_proto::TimerServiceClient>(slug).await?;
    let mut sessions = client
        .list_sessions(timer_proto::WorkSessionFilter::default())
        .await
        .map_err(|e| format!("{slug}: list sessions: {e:?}"))?;
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    Ok(sessions)
}

/// Sessions across several orgs, slug-tagged, newest first — the hours
/// that become billable lines.
pub async fn fetch_sessions_multi(slugs: &[String]) -> Vec<(String, timer_proto::WorkSession)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(sessions) = fetch_org_sessions(slug).await {
            out.extend(sessions.into_iter().map(|s| (slug.clone(), s)));
        }
    }
    out.sort_by(|a, b| b.1.start_time.cmp(&a.1.start_time));
    out
}

// ─────────────────────────────────────────────────────────────────────
// Store
// ─────────────────────────────────────────────────────────────────────

task_stores::stores! {
    InvoiceStore: OrgInvoice {
        provide: provide_invoice_store,
        handle: use_invoice_store,
    }
}

// ── invoices (multi-org, slug-tagged) ───────────────────────────────

/// Reactivity key for the *derived* uninvoiced-time view: settled
/// invoice mutations invalidate it, refreshing the aggregate the store
/// can't reconcile itself.
pub const UNINVOICED_KEY: &str = "finance.uninvoiced";

/// One invoice tagged with its owning org's slug.
#[derive(Clone, PartialEq)]
pub struct OrgInvoice {
    pub slug: String,
    pub invoice: finance_proto::Invoice,
}

impl StoreEntity for OrgInvoice {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.invoice.id
    }
}

/// Invoices across the selected orgs, newest first.
pub fn use_invoice_list() -> AtomResult<Vec<(Id<Uuid>, OrgInvoice)>, String> {
    use_multi_org_list(use_invoice_store(), |slugs| async move {
        Ok(self::fetch_invoices_multi(&slugs)
            .await
            .into_iter()
            .map(|(slug, invoice)| OrgInvoice { slug, invoice })
            .collect())
    })
}

/// Unsaved placeholder for an optimistic draft-invoice generation,
/// seeded with the uninvoiced group's totals. The server's generated
/// invoice (line items, party, book) reconciles in.
pub fn draft_invoice(amount_minor: i64, currency: String) -> finance_proto::Invoice {
    use finance_proto::invoice::{InvoiceKind, InvoiceStatus};
    let now = Utc::now();
    finance_proto::Invoice {
        id: Uuid::nil(),
        book_id: Uuid::nil(),
        party_id: Uuid::nil(),
        kind: InvoiceKind::Invoice,
        number: String::new(),
        status: InvoiceStatus::Draft,
        issue_date: now.date_naive().to_string(),
        due_date: String::new(),
        currency,
        exchange_rate_micro: 1_000_000,
        line_items: finance_proto::invoice::InvoiceLineItems::default(),
        invoice_taxes: finance_proto::TaxLines::default(),
        uses_inclusive_taxes: false,
        subtotal_minor: amount_minor,
        tax_total_minor: 0,
        total_minor: amount_minor,
        amount_paid_minor: 0,
        balance_minor: amount_minor,
        notes_public: String::new(),
        notes_private: String::new(),
        terms: String::new(),
        footer: String::new(),
        locked: false,
        posted_at: chrono::DateTime::<Utc>::UNIX_EPOCH,
        created_at: now,
        updated_at: now,
    }
}

#[derive(Clone, Copy)]
pub struct InvoiceMutations {
    store: InvoiceStore,
    write: Mutation<String>,
}

pub fn use_invoice_mutations() -> InvoiceMutations {
    InvoiceMutations {
        store: use_invoice_store(),
        // Every settled invoice write reshapes the uninvoiced view
        // (generate consumes groups; delete un-bills sessions).
        write: use_mutation().invalidating(&[UNINVOICED_KEY]),
    }
}

impl InvoiceMutations {
    /// Generate a draft invoice from an uninvoiced group: a draft row
    /// (with the group's totals) appears instantly, then reconciles to
    /// the server's generated invoice.
    pub fn generate(
        &self,
        slug: String,
        req: finance_proto::GenerateInvoice,
        amount_minor: i64,
        currency: String,
    ) {
        let row = OrgInvoice {
            slug: slug.clone(),
            invoice: draft_invoice(amount_minor, currency),
        };
        run_create(self.write, self.store, row, move |_| async move {
            self::generate_invoice(&slug, req)
                .await
                .map(|invoice| OrgInvoice {
                    slug: slug.clone(),
                    invoice,
                })
        });
    }

    /// Issue an invoice (assign number, lock).
    pub fn mark_sent(&self, slug: String, id: Uuid) {
        self.write.run(
            self.store,
            move |s| {
                s.update_optimistic(Id::Real(id), |r| {
                    r.invoice.status = finance_proto::invoice::InvoiceStatus::Sent;
                    r.invoice.locked = true;
                })
            },
            move || async move {
                self::invoice_mark_sent(&slug, id)
                    .await
                    .map(|invoice| Some(OrgInvoice { slug, invoice }))
            },
        );
    }

    /// Record a payment against an invoice.
    pub fn record_payment(&self, slug: String, id: Uuid, amount_minor: i64, date: String) {
        self.write.run(
            self.store,
            move |s| {
                s.update_optimistic(Id::Real(id), move |r| {
                    r.invoice.amount_paid_minor += amount_minor;
                    r.invoice.balance_minor = r.invoice.total_minor - r.invoice.amount_paid_minor;
                    if r.invoice.balance_minor <= 0 {
                        r.invoice.status = finance_proto::invoice::InvoiceStatus::Paid;
                    } else {
                        r.invoice.status = finance_proto::invoice::InvoiceStatus::PartiallyPaid;
                    }
                })
            },
            move || async move {
                self::invoice_record_payment(&slug, id, amount_minor, date)
                    .await
                    .map(|invoice| Some(OrgInvoice { slug, invoice }))
            },
        );
    }

    /// Delete a draft invoice (un-bills its sessions).
    pub fn delete(&self, slug: String, id: Uuid) {
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(id)),
            move || async move { self::invoice_delete(&slug, id).await.map(|()| None) },
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// The finances screen
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Period {
    Week,
    Month,
    All,
}

impl Period {
    fn label(self) -> &'static str {
        match self {
            Self::Week => "Last 7 days",
            Self::Month => "This month",
            Self::All => "All time",
        }
    }
}

fn hours(secs: i64) -> String {
    format!("{:.1}h", secs as f64 / 3600.0)
}

#[component]
pub fn FinancesView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let mut period = use_signal(|| Period::All);

    let slugs =
        use_memo(move || task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read()));

    let sessions = use_resource(move || async move { self::fetch_sessions_multi(&slugs()).await });
    // Project id → title, for the by-project breakdown.
    let projects = use_resource(move || async move {
        self::fetch_projects(&slugs())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect::<HashMap<Uuid, String>>()
    });

    let rows = sessions.read().clone().unwrap_or_default();
    let proj_names = projects.read().clone().unwrap_or_default();

    // Period cutoff (rolling windows; All = no cutoff).
    let today = Utc::now().date_naive();
    let in_period = |d: chrono::NaiveDate| match period() {
        Period::All => true,
        Period::Week => (today - d).num_days() < 7,
        Period::Month => d.year() == today.year() && d.month() == today.month(),
    };

    // Roll up closed sessions in the period.
    let mut total_secs = 0i64;
    let mut billable_secs = 0i64;
    let mut billable_cents = 0i64;
    let mut count = 0usize;
    let mut by_project: HashMap<Option<Uuid>, (i64, i64)> = HashMap::new();
    let mut by_org: HashMap<String, (i64, i64)> = HashMap::new();
    for (slug, s) in &rows {
        let Some(end) = s.end_time else { continue };
        if !in_period(s.start_time.date_naive()) {
            continue;
        }
        let secs = (end - s.start_time).num_seconds().max(0);
        let cents = if s.billable {
            secs * s.rate_cents / 3600
        } else {
            0
        };
        total_secs += secs;
        count += 1;
        if s.billable {
            billable_secs += secs;
            billable_cents += cents;
        }
        let p = by_project.entry(s.project_id).or_default();
        p.0 += secs;
        p.1 += cents;
        let o = by_org.entry(slug.clone()).or_default();
        o.0 += secs;
        o.1 += cents;
    }

    // Sort breakdowns by revenue desc.
    let mut proj_rows: Vec<(String, i64, i64)> = by_project
        .into_iter()
        .map(|(pid, (sec, cent))| {
            let name = pid.map_or_else(
                || "Unassigned".to_string(),
                |id| {
                    proj_names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "Unknown project".into())
                },
            );
            (name, sec, cent)
        })
        .collect();
    proj_rows.sort_by(|a, b| b.2.cmp(&a.2));
    let mut org_rows: Vec<(String, i64, i64)> = by_org
        .into_iter()
        .map(|(slug, (sec, cent))| {
            let name = org_list
                .read()
                .iter()
                .find(|o| o.slug == slug)
                .map_or(slug.clone(), |o| o.name.clone());
            (name, sec, cent)
        })
        .collect();
    org_rows.sort_by(|a, b| b.2.cmp(&a.2));

    let loading = sessions.read().is_none();

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            header { class: "flex items-center justify-between gap-3",
                div { class: "flex flex-col gap-1",
                    span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        "Billing"
                    }
                    Heading { level: HeadingLevel::H1, class: "tracking-tight", "Finances" }
                }
                div { class: "flex gap-1",
                    for p in [Period::Week, Period::Month, Period::All] {
                        Button {
                            key: "{p.label()}",
                            variant: if period() == p { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                            size: ButtonSize::Small,
                            on_click: move |_| period.set(p),
                            "{p.label()}"
                        }
                    }
                }
            }

            if loading {
                Text { variant: TextVariant::Muted, "Loading…" }
            } else {
                // Summary cards.
                div { class: "grid grid-cols-2 gap-3 sm:grid-cols-4",
                    StatCard { label: "Billable", value: money(billable_cents), accent: true }
                    StatCard { label: "Billable hrs", value: hours(billable_secs), accent: false }
                    StatCard { label: "Total hrs", value: hours(total_secs), accent: false }
                    StatCard { label: "Sessions", value: "{count}".to_string(), accent: false }
                }

                // By project.
                if !proj_rows.is_empty() {
                    Breakdown { title: "By project".to_string(), rows: proj_rows }
                }
                // By org (only meaningful across multiple).
                if org_rows.len() > 1 {
                    Breakdown { title: "By organization".to_string(), rows: org_rows }
                }

                if count == 0 {
                    div { class: "rounded-lg border border-dashed border-border px-4 py-10 text-center",
                        Text { variant: TextVariant::Muted, "No tracked time in this period." }
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(label: String, value: String, accent: bool) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1 rounded-xl border border-border/60 bg-card/40 p-3",
            span { class: "text-xs text-muted-foreground", "{label}" }
            span {
                class: if accent {
                    "font-mono text-xl font-semibold tabular-nums text-emerald-400"
                } else {
                    "font-mono text-xl font-semibold tabular-nums text-foreground"
                },
                "{value}"
            }
        }
    }
}

#[component]
fn Breakdown(title: String, rows: Vec<(String, i64, i64)>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-2",
            Heading { level: HeadingLevel::H3, "{title}" }
            div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                for (name , secs , cents) in rows {
                    div { key: "{name}", class: "flex items-center justify-between gap-3 px-3 py-2.5",
                        span { class: "truncate text-sm text-foreground", "{name}" }
                        div { class: "flex shrink-0 items-center gap-4",
                            span { class: "font-mono text-xs tabular-nums text-muted-foreground", "{hours(secs)}" }
                            span { class: "font-mono text-sm tabular-nums text-foreground", "{money(cents)}" }
                        }
                    }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// The invoices screen
// ─────────────────────────────────────────────────────────────────────

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
    let slugs =
        use_memo(move || task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read()));
    let mut selected = use_signal(|| None::<Uuid>);

    // The shared optimistic store: one AtomResult for the invoice list.
    let invoices = self::use_invoice_list();

    // Uninvoiced time is *derived* server-side (it reshapes whenever an
    // invoice is generated/deleted), so it can't reconcile from the
    // store — settled invoice mutations invalidate this reactivity key
    // and the resource refetches.
    let reactivity = architect::try_use_reactivity();
    let uninvoiced = use_resource(move || {
        if let Some(r) = reactivity {
            r.track(self::UNINVOICED_KEY);
        }
        async move { self::fetch_uninvoiced_multi(&slugs()).await }
    });
    let projects = use_resource(move || async move {
        self::fetch_projects(&slugs())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect::<HashMap<Uuid, String>>()
    });
    // Contacts back the bill-to picker (active only).
    let contacts = use_resource(move || async move {
        self::fetch_contacts(&slugs().into_iter().next().unwrap_or_default())
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c: &Contact| !c.archived)
            .collect::<Vec<Contact>>()
    });

    let store = self::use_invoice_store();
    let un_rows = uninvoiced.read().clone().unwrap_or_default();
    let inv_rows: Vec<(architect::Id<Uuid>, self::OrgInvoice)> =
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
                    task_ui_core::states::LoadingState {}
                } else if inv_rows.is_empty() {
                    if let Some(err) = load_err {
                        task_ui_core::states::ErrorState {
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
    let muts = self::use_invoice_mutations();
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
    let muts = self::use_invoice_mutations();
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

// ─────────────────────────────────────────────────────────────────────
// The ledger screen
// ─────────────────────────────────────────────────────────────────────

/// Display label for an account category.
fn kind_label(k: &AccountKind) -> &'static str {
    match k {
        AccountKind::Asset => "Assets",
        AccountKind::Liability => "Liabilities",
        AccountKind::Equity => "Equity",
        AccountKind::Income => "Income",
        AccountKind::Expense => "Expenses",
    }
}

/// Stable ordering for the account-category sections.
fn kind_order(k: &AccountKind) -> u8 {
    match k {
        AccountKind::Asset => 0,
        AccountKind::Liability => 1,
        AccountKind::Equity => 2,
        AccountKind::Income => 3,
        AccountKind::Expense => 4,
    }
}

#[component]
pub fn LedgerView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let slugs =
        use_memo(move || task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read()));

    // One org at a time keeps the double-entry view coherent (balances
    // are per-book). Use the first selected slug.
    let slug = use_memo(move || slugs().first().cloned());

    // Keep the fetch `Result` intact (no in-closure `unwrap_or_default`)
    // so a failed book read surfaces an error + retry instead of a
    // silently-blank ledger.
    let mut accounts = use_resource(move || async move {
        match slug() {
            Some(s) => self::fetch_ledger_accounts(&s).await,
            None => Ok(Vec::new()),
        }
    });
    let mut transactions = use_resource(move || async move {
        match slug() {
            Some(s) => self::fetch_ledger_transactions(&s).await,
            None => Ok(Vec::new()),
        }
    });

    let acct_res = accounts.read().clone();
    let txn_res = transactions.read().clone();
    // First error from either fetch (if any), and whether we're still
    // waiting on a selected org's data.
    let error = match (&acct_res, &txn_res) {
        (Some(Err(e)), _) | (_, Some(Err(e))) => Some(e.clone()),
        _ => None,
    };
    let loading = slug().is_some() && (acct_res.is_none() || txn_res.is_none());
    let acct_rows: Vec<(Account, AccountBalance)> =
        acct_res.and_then(Result::ok).unwrap_or_default();
    let txn_rows: Vec<Transaction> = txn_res.and_then(Result::ok).unwrap_or_default();

    // Account-name lookup for rendering splits in the transactions table.
    let acct_names: HashMap<Uuid, String> = acct_rows
        .iter()
        .map(|(a, _)| (a.id, a.name.clone()))
        .collect();

    // Group accounts by kind, ordered.
    let mut by_kind: Vec<(AccountKind, Vec<(Account, AccountBalance)>)> = Vec::new();
    for (a, b) in &acct_rows {
        match by_kind.iter_mut().find(|(k, _)| *k == a.kind) {
            Some((_, v)) => v.push((a.clone(), b.clone())),
            None => by_kind.push((a.kind, vec![(a.clone(), b.clone())])),
        }
    }
    by_kind.sort_by_key(|(k, _)| kind_order(k));

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Accounting"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Ledger" }
            }

            if slug().is_none() {
                div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                    Text { variant: TextVariant::Muted, "Select an org to view its ledger." }
                }
            } else if let Some(e) = error {
                task_ui_core::states::ErrorState {
                    title: "Couldn't load the ledger",
                    message: e,
                    on_retry: move |()| {
                        accounts.restart();
                        transactions.restart();
                    },
                }
            } else if loading {
                task_ui_core::states::LoadingState {}
            } else {
                // ── Accounts + balances ────────────────────────────
                div { class: "flex flex-col gap-2",
                    Heading { level: HeadingLevel::H3, "Accounts" }
                    if acct_rows.is_empty() {
                        div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                            Text { variant: TextVariant::Muted,
                                "No accounts yet — issue or pay an invoice to seed the ledger."
                            }
                        }
                    } else {
                        div { class: "flex flex-col gap-4",
                            for (kind , rows) in by_kind {
                                div { key: "{kind_label(&kind)}", class: "flex flex-col gap-1",
                                    span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.14em] text-muted-foreground",
                                        "{kind_label(&kind)}"
                                    }
                                    div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                                        for (acct , bal) in rows {
                                            div {
                                                key: "{acct.id}",
                                                class: "flex items-center justify-between gap-3 px-3 py-2",
                                                div { class: "flex min-w-0 flex-col",
                                                    span { class: "truncate text-sm text-foreground", "{acct.name}" }
                                                    if !bal.currency.is_empty() {
                                                        span { class: "text-xs text-muted-foreground", "{bal.currency}" }
                                                    }
                                                }
                                                span { class: "shrink-0 font-mono text-sm tabular-nums text-foreground",
                                                    "{money(bal.balance_minor)}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Recent transactions ────────────────────────────
                div { class: "flex flex-col gap-2",
                    Heading { level: HeadingLevel::H3, "Recent transactions" }
                    if txn_rows.is_empty() {
                        div { class: "rounded-lg border border-dashed border-border px-4 py-8 text-center",
                            Text { variant: TextVariant::Muted, "No journal entries yet." }
                        }
                    } else {
                        div { class: "flex flex-col gap-2",
                            for txn in txn_rows {
                                TransactionRow {
                                    key: "{txn.id}",
                                    txn: txn.clone(),
                                    acct_names: acct_names.clone(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TransactionRow(txn: Transaction, acct_names: HashMap<Uuid, String>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-2 rounded-lg border border-border/60 bg-card/40 px-3 py-2.5",
            div { class: "flex items-baseline justify-between gap-3",
                div { class: "flex min-w-0 flex-col",
                    span { class: "truncate text-sm font-medium text-foreground", "{txn.description}" }
                    if !txn.reference.is_empty() {
                        span { class: "text-xs text-muted-foreground", "Ref {txn.reference}" }
                    }
                }
                span { class: "shrink-0 text-xs text-muted-foreground", "{txn.date}" }
            }
            // Split lines: debits left, credits right (per
            // `TransactionSplit`: positive = debit, negative = credit).
            div { class: "flex flex-col gap-0.5",
                for (i , s) in txn.splits.0.iter().enumerate() {
                    div {
                        key: "{i}",
                        class: "flex items-center justify-between gap-3 text-xs",
                        span { class: "min-w-0 truncate text-muted-foreground",
                            {acct_names.get(&s.account_id).cloned().unwrap_or_else(|| "(account)".into())}
                        }
                        div { class: "flex shrink-0 items-center gap-6 font-mono tabular-nums",
                            // Debit column.
                            span { class: "w-20 text-right text-foreground",
                                if s.amount_minor > 0 { "{money(s.amount_minor)}" }
                            }
                            // Credit column.
                            span { class: "w-20 text-right text-foreground",
                                if s.amount_minor < 0 { "{money(-s.amount_minor)}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
