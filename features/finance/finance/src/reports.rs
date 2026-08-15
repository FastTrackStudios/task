//! Aggregate queries over the timer + finance stores.
//!
//! Two backing data sources:
//!
//! - **Timer** (`timer_work_sessions` rows) — billable hours,
//!   per-project rollups, per-client breakdowns. Owned by
//!   `timer::Store`; we just pass the connection in.
//! - **Finance** (`finance_invoices` + `finance_payments` rows)
//!   — accounts receivable, paid vs. unpaid. Owned by
//!   `finance-db`.
//!
//! All money values are integer minor units (cents) — pre-
//! formatting for display is the caller's job.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::finance_db_entity::{InvoiceColumn, InvoiceEntity};
use timer::entity::{WorkSessionColumn, WorkSessionEntity};

/// One row of the "hours by project" report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectRollup {
    pub project_id: Option<Uuid>,
    /// Snapshot project path (for display). Empty when no
    /// project link.
    pub project_path: String,
    pub session_count: u64,
    pub total_seconds: i64,
    pub billable_seconds: i64,
    /// Billable minor units, summed via the **snapshotted**
    /// `rate_cents` on each closed session — so retroactive
    /// rate changes don't move historical totals.
    pub billable_amount_minor: i64,
    /// Currency from the first billable session (rolls of
    /// projects with mixed currencies are caller's problem;
    /// in practice one project = one currency).
    pub currency: String,
}

/// Range filter for report queries. `until` is exclusive.
#[derive(Debug, Clone, Copy)]
pub struct DateRange {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl DateRange {
    /// The rolling 7-day window ending now.
    #[must_use]
    pub fn last_7_days() -> Self {
        let until = Utc::now();
        Self {
            since: until - Duration::days(7),
            until,
        }
    }

    /// The rolling 30-day window ending now.
    #[must_use]
    pub fn last_30_days() -> Self {
        let until = Utc::now();
        Self {
            since: until - Duration::days(30),
            until,
        }
    }

    /// Monday→Monday week containing `day` (in UTC).
    #[must_use]
    pub fn week_of(day: NaiveDate) -> Self {
        use chrono::Datelike;
        let dow_from_mon: i64 = i64::from(day.weekday().num_days_from_monday());
        let monday = day - Duration::days(dow_from_mon);
        let start_naive = monday.and_hms_opt(0, 0, 0).unwrap();
        let since = DateTime::<Utc>::from_naive_utc_and_offset(start_naive, Utc);
        Self {
            since,
            until: since + Duration::days(7),
        }
    }
}

/// Group closed WorkSessions by `project_id` over `range`.
/// Open sessions (no `end_time`) are excluded so the rollup
/// reflects only completed work.
pub async fn hours_by_project(
    conn: &DatabaseConnection,
    user_id: Option<Uuid>,
    range: DateRange,
) -> Result<Vec<ProjectRollup>, sea_orm::DbErr> {
    let mut q = WorkSessionEntity::find()
        .filter(WorkSessionColumn::StartTime.gte(range.since))
        .filter(WorkSessionColumn::StartTime.lt(range.until))
        .filter(WorkSessionColumn::EndTime.is_not_null());
    if let Some(uid) = user_id {
        q = q.filter(WorkSessionColumn::UserId.eq(uid));
    }
    let rows = q.all(conn).await?;
    Ok(rollup_sessions(
        rows.into_iter().map(timer_proto::WorkSession::from),
    ))
}

/// Pure fold behind [`hours_by_project`], over the wire shape —
/// so callers that fetch their sessions over vox
/// (`TimerService::list_sessions`) get the identical rollup
/// without a DB connection. Open sessions (no `end_time`) are
/// skipped.
pub fn rollup_sessions(
    sessions: impl IntoIterator<Item = timer_proto::WorkSession>,
) -> Vec<ProjectRollup> {
    let mut by_project: std::collections::BTreeMap<(Option<Uuid>, String), ProjectRollup> =
        std::collections::BTreeMap::new();
    for s in sessions {
        let Some(end) = s.end_time else { continue };
        let key = (s.project_id, s.project_path.clone());
        let elapsed = end.signed_duration_since(s.start_time).num_seconds().max(0);
        // billable amount = (elapsed_seconds / 3600) * rate_cents
        let billable_cents = if s.billable {
            (i128::from(elapsed) * i128::from(s.rate_cents) / 3600)
                .try_into()
                .unwrap_or(0)
        } else {
            0_i64
        };
        let bucket = by_project.entry(key).or_insert_with(|| ProjectRollup {
            project_id: s.project_id,
            project_path: s.project_path.clone(),
            session_count: 0,
            total_seconds: 0,
            billable_seconds: 0,
            billable_amount_minor: 0,
            currency: String::new(),
        });
        bucket.session_count += 1;
        bucket.total_seconds += elapsed;
        if s.billable {
            bucket.billable_seconds += elapsed;
            bucket.billable_amount_minor += billable_cents;
            if bucket.currency.is_empty() {
                bucket.currency = s.currency.clone();
            }
        }
    }
    let mut out: Vec<ProjectRollup> = by_project.into_values().collect();
    out.sort_by(|a, b| b.total_seconds.cmp(&a.total_seconds));
    out
}

/// Weekly summary — hours per project + invoice totals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklySummary {
    pub week_of: NaiveDate,
    pub projects: Vec<ProjectRollup>,
    pub total_seconds: i64,
    pub total_billable_seconds: i64,
    pub total_billable_amount_minor: i64,
    /// Currency of the **first** billable session in the week.
    /// Sums across mixed-currency projects are not converted;
    /// the report just surfaces the dominant one.
    pub currency: String,
}

impl WeeklySummary {
    /// Render as markdown — drops into a project page or
    /// daily note verbatim.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "## Time tracked — week of {}\n\n",
            self.week_of.format("%Y-%m-%d"),
        );
        if self.projects.is_empty() {
            out.push_str("_(no sessions tracked this week)_\n");
            return out;
        }
        out.push_str("| Project | Sessions | Total | Billable | Amount |\n");
        out.push_str("|---|---:|---:|---:|---:|\n");
        for p in &self.projects {
            out.push_str(&format!(
                "| {project} | {count} | {total} | {bill} | {amt} {cur} |\n",
                project = if p.project_path.is_empty() {
                    "(unscoped)".to_string()
                } else {
                    p.project_path.clone()
                },
                count = p.session_count,
                total = fmt_duration_secs(p.total_seconds),
                bill = fmt_duration_secs(p.billable_seconds),
                amt = fmt_minor(p.billable_amount_minor),
                cur = p.currency,
            ));
        }
        out.push_str(&format!(
            "| **Totals** | | **{total}** | **{bill}** | **{amt} {cur}** |\n",
            total = fmt_duration_secs(self.total_seconds),
            bill = fmt_duration_secs(self.total_billable_seconds),
            amt = fmt_minor(self.total_billable_amount_minor),
            cur = self.currency,
        ));
        out
    }
}

pub async fn weekly_summary(
    timer_conn: &DatabaseConnection,
    user_id: Option<Uuid>,
    week_of: NaiveDate,
) -> Result<WeeklySummary, sea_orm::DbErr> {
    let range = DateRange::week_of(week_of);
    let projects = hours_by_project(timer_conn, user_id, range).await?;
    Ok(weekly_summary_from_rollups(projects, range.since.date_naive()))
}

/// Pure totals fold behind [`weekly_summary`] — pair with
/// [`rollup_sessions`] when the sessions arrive over vox instead
/// of a DB connection. `week_of` should be the Monday the range
/// started ([`DateRange::week_of`]`.since.date_naive()`).
#[must_use]
pub fn weekly_summary_from_rollups(
    projects: Vec<ProjectRollup>,
    week_of: NaiveDate,
) -> WeeklySummary {
    let total_seconds: i64 = projects.iter().map(|p| p.total_seconds).sum();
    let total_billable_seconds: i64 = projects.iter().map(|p| p.billable_seconds).sum();
    let total_billable_amount_minor: i64 = projects.iter().map(|p| p.billable_amount_minor).sum();
    let currency = projects
        .iter()
        .find(|p| !p.currency.is_empty())
        .map(|p| p.currency.clone())
        .unwrap_or_default();
    WeeklySummary {
        week_of,
        projects,
        total_seconds,
        total_billable_seconds,
        total_billable_amount_minor,
        currency,
    }
}

// ── Accounts receivable ─────────────────────────────────────────────

/// One row in the AR aging report — outstanding balance per
/// client, bucketed by how overdue it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArAgingRow {
    pub party_id: Uuid,
    /// Sum of `balance_minor` across this party's open
    /// invoices. Always non-negative — credits are tracked
    /// on the party as separate `kind = "credit"` invoices.
    pub current_minor: i64,
    pub overdue_0_30_minor: i64,
    pub overdue_31_60_minor: i64,
    pub overdue_61_90_minor: i64,
    pub overdue_91_plus_minor: i64,
    pub total_outstanding_minor: i64,
    pub currency: String,
}

/// AR aging by client. Reads non-paid invoices from
/// `finance_invoices` and buckets each balance by the gap
/// between `today` and `due_date`.
pub async fn ar_aging(
    conn: &DatabaseConnection,
    book_id: Uuid,
    today: NaiveDate,
) -> Result<Vec<ArAgingRow>, sea_orm::DbErr> {
    let rows = InvoiceEntity::find()
        .filter(InvoiceColumn::BookId.eq(book_id))
        .filter(InvoiceColumn::Status.is_in(["sent", "draft"]))
        .filter(InvoiceColumn::BalanceMinor.gt(0))
        .order_by_asc(InvoiceColumn::DueDate)
        .all(conn)
        .await?;
    let mut by_party: std::collections::BTreeMap<Uuid, ArAgingRow> =
        std::collections::BTreeMap::new();
    for inv in rows {
        let due = NaiveDate::parse_from_str(&inv.due_date, "%Y-%m-%d").unwrap_or(today);
        let overdue_days = today.signed_duration_since(due).num_days();
        let bucket = by_party.entry(inv.party_id).or_insert_with(|| ArAgingRow {
            party_id: inv.party_id,
            current_minor: 0,
            overdue_0_30_minor: 0,
            overdue_31_60_minor: 0,
            overdue_61_90_minor: 0,
            overdue_91_plus_minor: 0,
            total_outstanding_minor: 0,
            currency: inv.currency.clone(),
        });
        let bal = inv.balance_minor;
        if overdue_days <= 0 {
            bucket.current_minor += bal;
        } else if overdue_days <= 30 {
            bucket.overdue_0_30_minor += bal;
        } else if overdue_days <= 60 {
            bucket.overdue_31_60_minor += bal;
        } else if overdue_days <= 90 {
            bucket.overdue_61_90_minor += bal;
        } else {
            bucket.overdue_91_plus_minor += bal;
        }
        bucket.total_outstanding_minor += bal;
    }
    Ok(by_party.into_values().collect())
}

// ── Helpers ────────────────────────────────────────────────────────

fn fmt_duration_secs(secs: i64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{m}m")
    }
}

fn fmt_minor(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let w = abs / 100;
    let f = abs % 100;
    format!("{}{w}.{f:02}", if neg { "-" } else { "" })
}
