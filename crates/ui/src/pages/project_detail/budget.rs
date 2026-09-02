//! Budget math — sessions + invoices folded into one snapshot.
//!
//! Pure data and pure functions (injected `now`), so every label the
//! page renders about money or time is testable without a component.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use timer_proto::WorkSession;
use uuid::Uuid;

/// Aggregated spend for one project, assembled client-side from the
/// timer + invoicing feeds.
#[derive(Clone, PartialEq, Default)]
pub(super) struct BudgetData {
    /// Σ session durations; an open timer counts up to "now".
    pub logged_seconds: i64,
    /// `(amount_minor, currency)` from the project's
    /// [`finance_proto::UninvoicedGroup`], when one exists.
    pub unbilled: Option<(i64, String)>,
    /// Σ `total_minor` of the invoices this project's sessions were
    /// billed on.
    pub invoiced_minor: i64,
    /// Σ `amount_paid_minor` of the same invoices.
    pub paid_minor: i64,
    /// Currency of those invoices (one project = one currency).
    pub invoice_currency: String,
    /// Whether the `session.invoice_id → invoice` join resolved any
    /// invoice at all — gates the Invoiced / Paid rows.
    pub has_invoices: bool,
}

/// Fold the project's sessions + the org's uninvoiced groups +
/// invoice list into one [`BudgetData`]. Pure (injected `now`) so the
/// duration math is testable.
///
/// The invoiced / paid join: invoices are generated per-project
/// (`GenerateInvoice` takes a project), so summing the invoices
/// referenced by this project's `session.invoice_id`s is exact and
/// costs a single `list_invoices` call — cheap enough to resolve
/// client-side rather than descope.
pub(super) fn build_budget(
    pid: Uuid,
    sessions: &[WorkSession],
    uninvoiced: &[finance_proto::UninvoicedGroup],
    invoices: &[finance_proto::Invoice],
    now: DateTime<Utc>,
) -> BudgetData {
    let logged_seconds = sessions
        .iter()
        .map(|s| {
            (s.end_time.unwrap_or(now) - s.start_time)
                .num_seconds()
                .max(0)
        })
        .sum();
    let unbilled = uninvoiced
        .iter()
        .find(|g| g.project_id == Some(pid))
        .map(|g| (g.amount_minor, g.currency.clone()));
    let billed_on: BTreeSet<Uuid> = sessions.iter().filter_map(|s| s.invoice_id).collect();
    let mut out = BudgetData {
        logged_seconds,
        unbilled,
        ..Default::default()
    };
    for inv in invoices.iter().filter(|i| billed_on.contains(&i.id)) {
        out.has_invoices = true;
        out.invoiced_minor += inv.total_minor;
        out.paid_minor += inv.amount_paid_minor;
        if out.invoice_currency.is_empty() {
            out.invoice_currency = inv.currency.clone();
        }
    }
    out
}

/// The masthead's budget text: "logged / estimated" in hours. The
/// caller renders "—" when there's no estimate at all.
pub(super) fn budget_tile_value(logged_seconds: i64, estimated_seconds: i64) -> String {
    format!(
        "{} / {}",
        hours_label(logged_seconds),
        hours_label(estimated_seconds)
    )
}

/// `amount_minor` + ISO currency → display string. Minor units are
/// hundredths (cents); an empty currency falls back to `$` like the
/// finances / invoices pages.
// Only its tests call it today; the budget table renders amounts through
// the finances helpers. Kept for the per-line view that is coming.
#[allow(dead_code)]
pub(super) fn money_label(amount_minor: i64, currency: &str) -> String {
    let amount = amount_minor as f64 / 100.0;
    if currency.is_empty() {
        format!("${amount:.2}")
    } else {
        format!("{amount:.2} {currency}")
    }
}

pub(super) fn hours_label(secs: i64) -> String {
    format!("{:.1}h", secs as f64 / 3600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_label_formats_minor_units_with_currency() {
        assert_eq!(money_label(123_456, "USD"), "1234.56 USD");
        assert_eq!(money_label(0, "EUR"), "0.00 EUR");
        // Empty currency falls back to `$` like the finances page.
        assert_eq!(money_label(995, ""), "$9.95");
        assert_eq!(money_label(-2_500, "USD"), "-25.00 USD");
    }

    #[test]
    fn budget_tile_shows_logged_vs_estimated_hours() {
        assert_eq!(budget_tile_value(5_400, 36_000), "1.5h / 10.0h");
        assert_eq!(budget_tile_value(0, 3_600), "0.0h / 1.0h");
    }

    #[test]
    fn hours_label_rounds_to_tenths() {
        assert_eq!(hours_label(3_600), "1.0h");
        assert_eq!(hours_label(5_430), "1.5h");
        assert_eq!(hours_label(0), "0.0h");
    }
}
