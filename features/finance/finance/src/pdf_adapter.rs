//! `finance_proto::Invoice` → an `InvoiceForPdf` struct
//! shape callers pass into the `pdf` crate's
//! `render_invoice`.
//!
//! Lives here (not in `pdf` or `finance-proto`) because the
//! marshalling logic is finance-domain (currency symbols,
//! tax-line collapsing, milli-hours → display hours) but
//! the rendering tool (fulgur) is too heavy to drag into the
//! finance compile graph — `finance` stays library-light, and
//! the CLI / server do the actual fulgur invocation.
//!
//! Field names + types intentionally mirror
//! `pdf::InvoiceData` so a consumer can move our struct
//! straight into the pdf-crate one.

use finance_proto::invoice::Invoice;
use finance_proto::party::Party;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct InvoiceForPdf {
    pub number: String,
    pub currency: String,
    pub currency_symbol: String,
    pub issue_date: String,
    pub due_date: String,
    pub period_start: String,
    pub period_end: String,
    pub status: String,
    pub from: PartyForPdf,
    pub to: PartyForPdf,
    pub lines: Vec<InvoiceLineForPdf>,
    pub subtotal: String,
    pub tax: String,
    pub total: String,
    pub balance_due: String,
    pub notes: String,
    pub terms: String,
    pub footer: String,
    /// Per-assignee aggregate rows for the summary block +
    /// charts. Empty = caller didn't bother computing them
    /// (single-assignee invoices generally skip this).
    pub assignees: Vec<AssigneeSummary>,
    /// Per-person concise summary block (hours + amount),
    /// rendered between the overview and the line-item
    /// detail. Empty list hides the block.
    pub people: Vec<AssigneeSummary>,
    /// Pre-rendered donut SVG (hours share). Empty string
    /// suppresses the chart container in the template.
    pub donut_svg: String,
    /// Pre-rendered horizontal bar SVG (amount per
    /// assignee). Same suppression rule.
    pub bars_svg: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PartyForPdf {
    pub name: String,
    pub address: String,
    pub email: String,
    pub phone: String,
    pub tax_id: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InvoiceLineForPdf {
    pub description: String,
    pub quantity: String,
    pub unit: String,
    pub unit_price: String,
    pub amount: String,
    /// Display name of the team member who logged this line.
    /// Empty = single-assignee invoice (column hidden).
    pub assignee: String,
}

/// One row of the per-assignee summary table — also the
/// data the donut + bar charts consume.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AssigneeSummary {
    pub name: String,
    pub hours: String,
    pub amount: String,
    /// `0.0..=100.0`, pre-formatted as a string like
    /// `"34.5"`.
    pub pct: String,
    /// Hex colour matching the chart slice / bar.
    pub color: String,
}

/// The issuer side ("From" header) — your business identity.
/// Pulled separately because the [`Party`] entity models the
/// counterparty (clients + vendors), not the user. The
/// issuer profile lives in the book's settings JSON or the
/// architect-auth user record.
#[derive(Debug, Clone)]
pub struct IssuerProfile {
    pub name: String,
    pub address: String,
    pub email: String,
    pub phone: String,
    pub tax_id: String,
}

#[must_use]
pub fn invoice_for_pdf(
    invoice: &Invoice,
    issuer: &IssuerProfile,
    bill_to: &Party,
) -> InvoiceForPdf {
    let lines = invoice
        .line_items
        .0
        .iter()
        .map(|li| InvoiceLineForPdf {
            description: li.description.clone(),
            quantity: format_milli_hours(li.quantity_milli),
            unit: if li.quantity_milli > 0 {
                "hr".into()
            } else {
                String::new()
            },
            unit_price: format_minor(li.unit_price_minor),
            amount: format_minor(li.line_total_minor),
            assignee: String::new(),
        })
        .collect();
    InvoiceForPdf {
        number: invoice.number.clone(),
        currency: invoice.currency.clone(),
        currency_symbol: currency_symbol(&invoice.currency).into(),
        issue_date: invoice.issue_date.clone(),
        due_date: invoice.due_date.clone(),
        period_start: String::new(),
        period_end: String::new(),
        status: status_slug(&invoice.status).into(),
        from: PartyForPdf {
            name: issuer.name.clone(),
            address: issuer.address.clone(),
            email: issuer.email.clone(),
            phone: issuer.phone.clone(),
            tax_id: issuer.tax_id.clone(),
        },
        to: PartyForPdf {
            name: if bill_to.legal_name.is_empty() {
                bill_to.display_name.clone()
            } else {
                bill_to.legal_name.clone()
            },
            address: bill_to.address.clone(),
            email: bill_to.email.clone(),
            phone: bill_to.phone.clone(),
            tax_id: bill_to.tax_id.clone(),
        },
        lines,
        subtotal: format_minor(invoice.subtotal_minor),
        tax: if invoice.tax_total_minor == 0 {
            String::new()
        } else {
            format_minor(invoice.tax_total_minor)
        },
        total: format_minor(invoice.total_minor),
        balance_due: if invoice.balance_minor == 0 {
            String::new()
        } else {
            format_minor(invoice.balance_minor)
        },
        notes: invoice.notes_public.clone(),
        terms: invoice.terms.clone(),
        footer: invoice.footer.clone(),
        assignees: Vec::new(),
        people: Vec::new(),
        donut_svg: String::new(),
        bars_svg: String::new(),
    }
}

fn format_minor(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let w = abs / 100;
    let f = abs % 100;
    let w_s = thousands_grouped(w);
    format!("{}{w_s}.{f:02}", if neg { "-" } else { "" })
}

fn thousands_grouped(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut groups = Vec::new();
    while n > 0 {
        groups.push(n % 1000);
        n /= 1000;
    }
    groups.reverse();
    let mut out = groups[0].to_string();
    for g in &groups[1..] {
        out.push_str(&format!(",{g:03}"));
    }
    out
}

fn format_milli_hours(milli: i64) -> String {
    let whole = milli / 1000;
    let frac = (milli.abs() % 1000) / 10;
    format!("{}{whole}.{frac:02}", if milli < 0 { "-" } else { "" })
}

fn currency_symbol(code: &str) -> &'static str {
    match code {
        "USD" | "CAD" | "AUD" | "NZD" | "MXN" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" | "CNY" => "¥",
        "CHF" => "₣",
        _ => "",
    }
}

fn status_slug(s: &finance_proto::invoice::InvoiceStatus) -> &'static str {
    use finance_proto::invoice::InvoiceStatus;
    match s {
        InvoiceStatus::Draft => "draft",
        InvoiceStatus::Sent => "sent",
        InvoiceStatus::Paid => "paid",
        InvoiceStatus::PartiallyPaid | InvoiceStatus::Overdue | InvoiceStatus::Viewed => "sent",
        InvoiceStatus::Cancelled | InvoiceStatus::Reversed => "void",
    }
}
