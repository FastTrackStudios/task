//! Build an [`Invoice`] from a span of billable
//! [`WorkSession`]s.
//!
//! Pipeline:
//!
//! 1. Query the timer DB for billable, closed sessions in
//!    the requested range, scoped to a single project
//!    (so the invoice is per-engagement).
//! 2. Group rows into line items — one per task-note path,
//!    or one per session when no task-note link is set.
//! 3. Snapshot quantities + amounts at session-close rates
//!    (`rate_cents` + `currency`). Re-billing later won't
//!    move historical totals because rates are
//!    already-frozen on the session row.
//! 4. Build an [`Invoice`] row + [`InvoiceLineItem`]s; the
//!    caller chooses to insert via `finance-db` or just
//!    render to PDF on-demand.
//!
//! Cross-currency sessions in one bill are refused — the
//! invoice can carry exactly one currency. Mixed-currency
//! ranges return [`InvoicePipelineError::MixedCurrency`].

use chrono::{DateTime, Duration, NaiveDate, Utc};
use finance_proto::book::Book;
use finance_proto::invoice::{
    Invoice, InvoiceKind, InvoiceLineItem, InvoiceLineItems, InvoiceStatus, LineItemType,
};
use finance_proto::party::Party;
use finance_proto::tax::TaxLines;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use timer::entity::{WorkSessionColumn, WorkSessionEntity, WorkSessionModel};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum InvoicePipelineError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),

    /// Sessions in the range used more than one currency —
    /// invoices can't mix currencies. Caller's fix: invoice
    /// each currency separately.
    #[error("mixed currencies in session set: {0}")]
    MixedCurrency(String),

    /// No billable sessions matched.
    #[error("no billable sessions in range")]
    NothingToBill,

    #[error("invalid: {0}")]
    Invalid(String),
}

/// Args for [`build_invoice_from_sessions`].
#[derive(Debug, Clone)]
pub struct BuildInvoiceArgs {
    /// Owning book (controls invoice numbering, default
    /// currency, P&L attribution).
    pub book: Book,
    /// Billed party — generally the project's client.
    pub party: Party,
    /// Project filter. Required: invoices are
    /// per-engagement.
    pub project_id: Uuid,
    /// Inclusive lower bound on `start_time`.
    pub since: DateTime<Utc>,
    /// Exclusive upper bound on `start_time`.
    pub until: DateTime<Utc>,
    /// Net N days — `due_date = today + net_days`.
    pub net_days: i64,
    /// Invoice number to assign. Caller is responsible for
    /// uniqueness within the book (the DB unique index will
    /// reject collisions).
    pub number: String,
    /// Free-text public + private notes to attach.
    pub notes_public: String,
    pub notes_private: String,
    pub terms: String,
}

/// Result of the pipeline. Caller decides whether to
/// `finance-db::InvoiceEntity::insert` it + the linked
/// sessions, or just render to PDF for preview.
#[derive(Debug, Clone)]
pub struct InvoiceBuild {
    pub invoice: Invoice,
    /// Session ids that contributed to this invoice. Caller
    /// records these somewhere (custom join table or a
    /// future `WorkSession.invoice_id` column) so the same
    /// sessions don't get re-billed next run.
    pub source_session_ids: Vec<Uuid>,
    /// Sidecar metadata per line, keyed by the line's own
    /// `id`. Lives outside [`InvoiceLineItem`] because the
    /// proto schema has no assignee column. The renderer
    /// joins on this to label + chart by assignee.
    pub line_meta: Vec<LineMeta>,
}

/// Per-line sidecar — who logged it, how long, how much.
/// `secs` and `cents` mirror the line totals (kept so the
/// chart can skip re-deriving them from the formatted
/// quantity / amount strings).
#[derive(Debug, Clone, Copy)]
pub struct LineMeta {
    pub line_id: Uuid,
    pub user_id: Uuid,
    pub secs: i64,
    pub cents: i64,
}

pub async fn build_invoice_from_sessions(
    timer_conn: &DatabaseConnection,
    args: BuildInvoiceArgs,
) -> Result<InvoiceBuild, InvoicePipelineError> {
    let sessions: Vec<WorkSessionModel> = WorkSessionEntity::find()
        .filter(WorkSessionColumn::ProjectId.eq(args.project_id))
        .filter(WorkSessionColumn::Billable.eq(true))
        .filter(WorkSessionColumn::EndTime.is_not_null())
        // Only bill sessions not already on an invoice.
        .filter(WorkSessionColumn::InvoiceId.is_null())
        .filter(WorkSessionColumn::StartTime.gte(args.since))
        .filter(WorkSessionColumn::StartTime.lt(args.until))
        .all(timer_conn)
        .await?;
    build_from_models(
        args.book,
        args.party,
        sessions,
        args.net_days,
        args.number,
        args.notes_public,
        args.notes_private,
        args.terms,
    )
}

/// Build a draft invoice from an already-selected set of work-session
/// models (the caller decides the filter — by project, by tag, or all
/// general/project-less time). Same grouping + money logic as
/// [`build_invoice_from_sessions`].
#[allow(clippy::too_many_arguments)]
pub fn build_from_models(
    book: Book,
    party: Party,
    sessions: Vec<WorkSessionModel>,
    net_days: i64,
    number: String,
    notes_public: String,
    notes_private: String,
    terms: String,
) -> Result<InvoiceBuild, InvoicePipelineError> {
    if sessions.is_empty() {
        return Err(InvoicePipelineError::NothingToBill);
    }

    // Currency check — refuse mixed.
    let currency = sessions
        .iter()
        .find(|s| !s.currency.is_empty())
        .map(|s| s.currency.clone())
        .ok_or_else(|| {
            InvoicePipelineError::Invalid("no session had a currency snapshot".into())
        })?;
    if let Some(bad) = sessions
        .iter()
        .find(|s| !s.currency.is_empty() && s.currency != currency)
    {
        return Err(InvoicePipelineError::MixedCurrency(format!(
            "expected {currency:?}, found {:?}",
            bad.currency
        )));
    }

    // Group by (assignee, task-note path) so each user's
    // contribution to a task is its own line. Sessions
    // without a task note each become their own line so
    // users can spot stray work.
    let mut by_user_task: std::collections::BTreeMap<(Uuid, String), Vec<&WorkSessionModel>> =
        std::collections::BTreeMap::new();
    for s in &sessions {
        let key = if s.task_note_path.is_empty() {
            format!("__unscoped__/{}", s.id.simple())
        } else {
            s.task_note_path.clone()
        };
        by_user_task.entry((s.user_id, key)).or_default().push(s);
    }

    // Each line carries its earliest session start so the
    // finished invoice reads chronologically.
    let mut dated_lines: Vec<(DateTime<Utc>, InvoiceLineItem, LineMeta)> =
        Vec::with_capacity(by_user_task.len());
    for ((user_id, key), bucket) in by_user_task {
        let total_secs: i64 = bucket
            .iter()
            .map(|s| {
                s.end_time
                    .unwrap_or(s.start_time)
                    .signed_duration_since(s.start_time)
                    .num_seconds()
                    .max(0)
            })
            .sum();
        // milli-hours: 1 hour = 1_000. (Matches the
        // proto's `quantity_milli` convention.)
        let quantity_milli = (i128::from(total_secs) * 1_000 / 3600) as i64;
        // Per-line unit rate = the first session's snapshotted
        // rate. If members are billed at different rates on
        // the same project, we'd need to split the line; v1
        // bails with a warning rather than silently mixing.
        let mut rates: std::collections::BTreeSet<i64> = bucket
            .iter()
            .filter(|s| s.rate_cents > 0)
            .map(|s| s.rate_cents)
            .collect();
        let unit_price_minor = rates.pop_last().unwrap_or(0);
        let multi_rate = rates.len() > 1;
        // line_total = sum(per-session billable cents) so
        // rounding adds at the session boundary, not the line.
        let mut line_total_minor: i64 = 0;
        for s in &bucket {
            let secs = s
                .end_time
                .unwrap_or(s.start_time)
                .signed_duration_since(s.start_time)
                .num_seconds()
                .max(0);
            let cents = (i128::from(secs) * i128::from(s.rate_cents) / 3600)
                .try_into()
                .unwrap_or(0_i64);
            line_total_minor += cents;
        }
        let line_secs = total_secs;
        let description = if let Some(stripped) = key.strip_prefix("__unscoped__/") {
            // Use the first session's description verbatim.
            bucket.first().map_or_else(
                || key.clone(),
                |s| {
                    if s.description.is_empty() {
                        format!("Session {stripped}")
                    } else {
                        s.description.clone()
                    }
                },
            )
        } else {
            // task-note-path → e.g. "tasks/port-kanban.md"; we
            // present just the stem for the customer.
            let stem = std::path::Path::new(&key)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&key)
                .replace('-', " ");
            if multi_rate {
                format!("{stem} (mixed rates)")
            } else {
                stem
            }
        };
        // Prefix the work date (a range when the bucket spans
        // days) so the customer can match lines to their own
        // calendar. FUTURE: a real date column on
        // `pdf::InvoiceLine` + the template once the proto's
        // `InvoiceLineItem` grows a date field.
        let first_start = bucket
            .iter()
            .map(|s| s.start_time)
            .min()
            .unwrap_or_else(Utc::now);
        let last_start = bucket
            .iter()
            .map(|s| s.start_time)
            .max()
            .unwrap_or(first_start);
        let (first_day, last_day) = (first_start.date_naive(), last_start.date_naive());
        let date_prefix = if first_day == last_day {
            first_day.format("%Y-%m-%d").to_string()
        } else {
            format!(
                "{} – {}",
                first_day.format("%Y-%m-%d"),
                last_day.format("%m-%d")
            )
        };
        let line_id = Uuid::new_v4();
        dated_lines.push((
            first_start,
            InvoiceLineItem {
                id: line_id,
                kind: LineItemType::TimeEntry,
                description: format!("{date_prefix}  {description}"),
                quantity_milli,
                unit_price_minor,
                discount_minor: 0,
                taxes: Vec::new(),
                line_total_minor,
                source_ref: bucket.first().map_or_else(Uuid::nil, |s| s.id),
            },
            LineMeta {
                line_id,
                user_id,
                secs: line_secs,
                cents: line_total_minor,
            },
        ));
    }
    dated_lines.sort_by_key(|(start, _, _)| *start);
    let mut lines: Vec<InvoiceLineItem> = Vec::with_capacity(dated_lines.len());
    let mut line_meta: Vec<LineMeta> = Vec::with_capacity(dated_lines.len());
    for (_, l, m) in dated_lines {
        lines.push(l);
        line_meta.push(m);
    }

    let subtotal: i64 = lines.iter().map(|l| l.line_total_minor).sum();
    let total = subtotal; // no tax pass yet — caller can add
    let now = Utc::now();
    let today = now.date_naive();
    let due = (today + Duration::days(net_days))
        .format("%Y-%m-%d")
        .to_string();

    let invoice = Invoice {
        id: Uuid::new_v4(),
        book_id: book.id,
        party_id: party.id,
        kind: InvoiceKind::Invoice,
        number,
        status: InvoiceStatus::Draft,
        issue_date: today.format("%Y-%m-%d").to_string(),
        due_date: due,
        currency,
        exchange_rate_micro: 1_000_000, // identity
        line_items: InvoiceLineItems(lines),
        invoice_taxes: TaxLines(Vec::new()),
        uses_inclusive_taxes: false,
        subtotal_minor: subtotal,
        tax_total_minor: 0,
        total_minor: total,
        amount_paid_minor: 0,
        balance_minor: total,
        notes_public,
        notes_private,
        terms,
        footer: String::new(),
        locked: false,
        posted_at: now,
        created_at: now,
        updated_at: now,
    };

    Ok(InvoiceBuild {
        invoice,
        source_session_ids: sessions.iter().map(|s| s.id).collect(),
        line_meta,
    })
}

/// Anchor a `today` date for tests / time-travelling
/// callers.
#[must_use]
pub fn naive_today() -> NaiveDate {
    Utc::now().date_naive()
}
