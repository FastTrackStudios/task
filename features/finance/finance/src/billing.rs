//! `FinanceBackend` — the mounted [`Invoicing`] service.
//!
//! Holds the org's finance + timer SQLite connections. Persists
//! invoices in `finance-db`, links billed sessions back via
//! `timer_work_sessions.invoice_id`, and answers the UI's
//! generate / list / pay / uninvoiced queries. The `Invoicing` trait
//! is sync, so (like `LedgerService`) each method `block_on`s a
//! captured runtime handle around the async DB work.
//!
//! MVP fidelity: persisted invoices with Draft → Sent → Paid /
//! PartiallyPaid status + simple numbering. Double-entry ledger
//! postings, taxes, and credit notes are deferred (the proto supports
//! them; the four unimplemented `Invoicing` methods return a backend
//! error for now).

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use finance_db::entity::{
    BookActive, BookColumn, BookEntity, BookModel, InvoiceActive, InvoiceColumn, InvoiceEntity,
    InvoiceModel, PartyActive, PartyColumn, PartyEntity, PartyModel,
};
use finance_proto::book::{Book, BookKind};
use finance_proto::error::FinanceError;
use finance_proto::invoice::{Invoice, InvoiceStatus};
use finance_proto::ledger::{AccountKind, TransactionSource, TransactionSplit, TransactionSplits};
use finance_proto::party::{Party, PartyKind};
use finance_proto::service::invoicing::{
    GenerateInvoice, Invoicing, RecordPayment, UninvoicedGroup,
};
use finance_proto::service::ledger::PostTransaction;
use std::collections::HashMap;

use crate::ledger::LedgerService;

use timer::entity::{
    TagColumn, TagEntity, WorkSessionColumn, WorkSessionEntity, WorkSessionTagColumn,
    WorkSessionTagEntity,
};

use crate::invoice_from_sessions::{
    BuildInvoiceArgs, build_from_models, build_invoice_from_sessions,
};

/// Disk-backed invoicing service for one org.
#[derive(Clone, architect::HasDispatcher)]
pub struct FinanceBackend {
    finance: DatabaseConnection,
    timer: DatabaseConnection,
    runtime: tokio::runtime::Handle,
    org_name: String,
    /// Double-entry ledger over the same `finance` connection. The
    /// invoicing flow posts journal entries here on mark-sent +
    /// payment so the ledger carries the real money movements.
    ledger: LedgerService,
}

impl FinanceBackend {
    /// Build over the org's finance + timer connections plus the
    /// ledger service (which wraps the same `finance` connection).
    /// Must be called from inside a tokio runtime.
    pub fn new(
        finance: DatabaseConnection,
        timer: DatabaseConnection,
        org_name: impl Into<String>,
        ledger: LedgerService,
    ) -> Result<Self, &'static str> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "FinanceBackend::new must be called from a tokio runtime")?;
        Ok(Self {
            finance,
            timer,
            runtime,
            org_name: org_name.into(),
            ledger,
        })
    }

    fn err(e: sea_orm::DbErr) -> FinanceError {
        FinanceError::Backend {
            message: e.to_string(),
        }
    }

    fn backend(msg: impl Into<String>) -> FinanceError {
        FinanceError::Backend {
            message: msg.into(),
        }
    }

    // ── book / party (find-or-create) ──────────────────────────────

    async fn ensure_book(&self) -> Result<Book, FinanceError> {
        if let Some(m) = BookEntity::find()
            .one(&self.finance)
            .await
            .map_err(Self::err)?
        {
            return Ok(model_to_book(m));
        }
        let now = Utc::now();
        let book = Book {
            id: Uuid::new_v4(),
            name: self.org_name.clone(),
            kind: BookKind::Business,
            base_currency: "USD".into(),
            settings_json: "{}".into(),
            created_at: now,
            updated_at: now,
        };
        BookEntity::insert(book_to_active(&book))
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        Ok(book)
    }

    async fn ensure_party(&self, book_id: Uuid, name: &str) -> Result<Party, FinanceError> {
        if let Some(m) = PartyEntity::find()
            .filter(PartyColumn::BookId.eq(book_id))
            .filter(PartyColumn::DisplayName.eq(name))
            .one(&self.finance)
            .await
            .map_err(Self::err)?
        {
            return Ok(model_to_party(m));
        }
        let now = Utc::now();
        let party = Party {
            id: Uuid::new_v4(),
            book_id,
            kind: PartyKind::Client,
            display_name: name.to_string(),
            legal_name: name.to_string(),
            email: String::new(),
            phone: String::new(),
            address: String::new(),
            tax_id: String::new(),
            default_currency: "USD".into(),
            default_net_days: 30,
            default_rate_minor_per_hour: 0,
            notes: String::new(),
            is_archived: false,
            created_at: now,
            updated_at: now,
        };
        PartyEntity::insert(party_to_active(&party))
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        Ok(party)
    }

    // ── ledger postings (double-entry) ─────────────────────────────
    //
    // Account model is the canonical `finance_proto::ledger::Account`
    // with `AccountKind`. Sign convention (per `TransactionSplit`):
    // positive `amount_minor` = debit, negative = credit; splits net
    // to zero in base currency. We use same-currency splits
    // (`fx_amount == amount`, `fx_rate_micro == 1_000_000`) — the
    // invoice total is already in the book's base currency.

    /// Build a same-currency split (debit when `amount_minor > 0`).
    fn split(account_id: Uuid, amount_minor: i64, memo: &str) -> TransactionSplit {
        TransactionSplit {
            account_id,
            amount_minor,
            fx_amount_minor: amount_minor,
            fx_rate_micro: 1_000_000,
            memo: memo.to_string(),
        }
    }

    /// On **mark-sent**: recognize revenue. Debit Accounts Receivable,
    /// credit Income for the invoice total. No-op for a zero total.
    async fn post_invoice_sent(&self, inv: &Invoice) -> Result<(), FinanceError> {
        if inv.total_minor == 0 {
            return Ok(());
        }
        let ar = self
            .ledger
            .ensure_account(inv.book_id, "Accounts Receivable", AccountKind::Asset)
            .await?;
        let income = self
            .ledger
            .ensure_account(inv.book_id, "Income", AccountKind::Income)
            .await?;
        let memo = format!("Invoice {}", inv.number);
        self.ledger
            .post(PostTransaction {
                book_id: inv.book_id,
                date: inv.issue_date.clone(),
                description: format!("Invoice {} issued", inv.number),
                reference: inv.number.clone(),
                source_kind: TransactionSource::Invoice,
                source_id: inv.id,
                splits: TransactionSplits(vec![
                    Self::split(ar, inv.total_minor, &memo),
                    Self::split(income, -inv.total_minor, &memo),
                ]),
            })
            .await?;
        Ok(())
    }

    /// On **record-payment**: settle receivable. Debit Cash, credit
    /// Accounts Receivable for the payment amount. No-op for zero.
    async fn post_invoice_payment(
        &self,
        inv: &Invoice,
        amount_minor: i64,
        date: &str,
    ) -> Result<(), FinanceError> {
        if amount_minor == 0 {
            return Ok(());
        }
        let cash = self
            .ledger
            .ensure_account(inv.book_id, "Cash", AccountKind::Asset)
            .await?;
        let ar = self
            .ledger
            .ensure_account(inv.book_id, "Accounts Receivable", AccountKind::Asset)
            .await?;
        let memo = format!("Payment on invoice {}", inv.number);
        self.ledger
            .post(PostTransaction {
                book_id: inv.book_id,
                date: date.to_string(),
                description: format!("Payment received on invoice {}", inv.number),
                reference: inv.number.clone(),
                source_kind: TransactionSource::Payment,
                source_id: inv.id,
                splits: TransactionSplits(vec![
                    Self::split(cash, amount_minor, &memo),
                    Self::split(ar, -amount_minor, &memo),
                ]),
            })
            .await?;
        Ok(())
    }

    // ── operations ─────────────────────────────────────────────────

    /// The alphabetically-first tag name per session (deterministic
    /// bucket key). Sessions with no tags map to `""`.
    async fn session_first_tag(&self, ids: &[Uuid]) -> Result<HashMap<Uuid, String>, FinanceError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let joins = WorkSessionTagEntity::find()
            .filter(WorkSessionTagColumn::WorkSessionId.is_in(ids.to_vec()))
            .all(&self.timer)
            .await
            .map_err(Self::err)?;
        let tag_ids: Vec<Uuid> = joins.iter().map(|j| j.tag_id).collect();
        let names: HashMap<Uuid, String> = TagEntity::find()
            .filter(TagColumn::Id.is_in(tag_ids))
            .all(&self.timer)
            .await
            .map_err(Self::err)?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let mut by_session: HashMap<Uuid, Vec<String>> = HashMap::new();
        for j in joins {
            if let Some(n) = names.get(&j.tag_id) {
                by_session
                    .entry(j.work_session_id)
                    .or_default()
                    .push(n.clone());
            }
        }
        Ok(by_session
            .into_iter()
            .map(|(sid, mut ns)| {
                ns.sort();
                (sid, ns.into_iter().next().unwrap_or_default())
            })
            .collect())
    }

    async fn generate_inner(&self, req: GenerateInvoice) -> Result<Invoice, FinanceError> {
        let book = self.ensure_book().await?;
        let party = self.ensure_party(book.id, req.client_name.trim()).await?;

        let since = parse_day(&req.since).unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap());
        let until = parse_day(&req.until)
            .map_or_else(|| Utc::now() + Duration::days(1), |d| d + Duration::days(1));
        let net_days = req.net_days;
        let terms = format!("Net {net_days} days from issue date.");

        let build = if let Some(pid) = req.project_id {
            build_invoice_from_sessions(
                &self.timer,
                BuildInvoiceArgs {
                    book,
                    party,
                    project_id: pid,
                    since,
                    until,
                    net_days,
                    number: String::new(),
                    notes_public: String::new(),
                    notes_private: String::new(),
                    terms,
                },
            )
            .await
            .map_err(|e| Self::backend(e.to_string()))?
        } else {
            // Tag / general: project-less billable, un-invoiced sessions
            // in range, bucketed by their first tag (`tag` empty =
            // untagged "General").
            let sessions = WorkSessionEntity::find()
                .filter(WorkSessionColumn::Billable.eq(true))
                .filter(WorkSessionColumn::EndTime.is_not_null())
                .filter(WorkSessionColumn::InvoiceId.is_null())
                .filter(WorkSessionColumn::ProjectId.is_null())
                .filter(WorkSessionColumn::StartTime.gte(since))
                .filter(WorkSessionColumn::StartTime.lt(until))
                .all(&self.timer)
                .await
                .map_err(Self::err)?;
            let ids: Vec<Uuid> = sessions.iter().map(|s| s.id).collect();
            let tag_map = self.session_first_tag(&ids).await?;
            let filtered: Vec<_> = sessions
                .into_iter()
                .filter(|s| {
                    let t = tag_map.get(&s.id).cloned().unwrap_or_default();
                    if req.tag.is_empty() {
                        t.is_empty()
                    } else {
                        t == req.tag
                    }
                })
                .collect();
            build_from_models(
                book,
                party,
                filtered,
                net_days,
                String::new(),
                String::new(),
                String::new(),
                terms,
            )
            .map_err(|e| Self::backend(e.to_string()))?
        };

        let invoice = build.invoice;
        InvoiceEntity::insert(invoice_to_active(&invoice))
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;

        // Mark the billed sessions so they don't get re-invoiced.
        if !build.source_session_ids.is_empty() {
            WorkSessionEntity::update_many()
                .col_expr(WorkSessionColumn::InvoiceId, Expr::value(invoice.id))
                .filter(WorkSessionColumn::Id.is_in(build.source_session_ids.clone()))
                .exec(&self.timer)
                .await
                .map_err(Self::err)?;
        }
        Ok(invoice)
    }

    async fn list_inner(&self) -> Result<Vec<Invoice>, FinanceError> {
        let rows = InvoiceEntity::find()
            .order_by_desc(InvoiceColumn::CreatedAt)
            .all(&self.finance)
            .await
            .map_err(Self::err)?;
        Ok(rows.into_iter().map(model_to_invoice).collect())
    }

    async fn get_inner(&self, id: Uuid) -> Result<Invoice, FinanceError> {
        InvoiceEntity::find_by_id(id)
            .one(&self.finance)
            .await
            .map_err(Self::err)?
            .map(model_to_invoice)
            .ok_or_else(|| FinanceError::NotFound { id: id.to_string() })
    }

    async fn delete_inner(&self, id: Uuid) -> Result<(), FinanceError> {
        let inv = self.get_inner(id).await?;
        if inv.status != InvoiceStatus::Draft {
            return Err(Self::backend(
                "only draft invoices can be deleted; void instead",
            ));
        }
        InvoiceEntity::delete_by_id(id)
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        // Un-bill its sessions.
        WorkSessionEntity::update_many()
            .col_expr(
                WorkSessionColumn::InvoiceId,
                Expr::value(Option::<Uuid>::None),
            )
            .filter(WorkSessionColumn::InvoiceId.eq(id))
            .exec(&self.timer)
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    async fn pay_inner(
        &self,
        id: Uuid,
        amount_minor: i64,
        date: String,
    ) -> Result<Invoice, FinanceError> {
        let mut inv = self.get_inner(id).await?;
        inv.amount_paid_minor += amount_minor;
        inv.balance_minor = inv.total_minor - inv.amount_paid_minor;
        inv.status = if inv.balance_minor <= 0 {
            InvoiceStatus::Paid
        } else {
            InvoiceStatus::PartiallyPaid
        };
        inv.updated_at = Utc::now();
        let mut active = invoice_to_active(&inv);
        active.id = sea_orm::ActiveValue::Unchanged(inv.id);
        active.update(&self.finance).await.map_err(Self::err)?;
        // Settle the receivable: debit Cash, credit AR for the payment.
        let date = if date.trim().is_empty() {
            Utc::now().date_naive().to_string()
        } else {
            date
        };
        self.post_invoice_payment(&inv, amount_minor, &date).await?;
        Ok(inv)
    }

    async fn mark_sent_inner(&self, id: Uuid) -> Result<Invoice, FinanceError> {
        let mut inv = self.get_inner(id).await?;
        if inv.status == InvoiceStatus::Draft {
            // Assign the next sequential number in the book.
            let issued = InvoiceEntity::find()
                .filter(InvoiceColumn::Number.ne(""))
                .all(&self.finance)
                .await
                .map_err(Self::err)?
                .len();
            inv.number = format!("INV-{}-{:04}", Utc::now().year(), issued + 1);
            inv.status = InvoiceStatus::Sent;
            inv.posted_at = Utc::now();
            inv.locked = true;
            inv.updated_at = Utc::now();
            let mut active = invoice_to_active(&inv);
            active.id = sea_orm::ActiveValue::Unchanged(inv.id);
            active.update(&self.finance).await.map_err(Self::err)?;
            // Recognize revenue: debit AR, credit Income for the total.
            self.post_invoice_sent(&inv).await?;
        }
        Ok(inv)
    }

    async fn uninvoiced_inner(&self) -> Result<Vec<UninvoicedGroup>, FinanceError> {
        let rows = WorkSessionEntity::find()
            .filter(WorkSessionColumn::Billable.eq(true))
            .filter(WorkSessionColumn::EndTime.is_not_null())
            .filter(WorkSessionColumn::InvoiceId.is_null())
            .all(&self.timer)
            .await
            .map_err(Self::err)?;
        let pl_ids: Vec<Uuid> = rows
            .iter()
            .filter(|s| s.project_id.is_none())
            .map(|s| s.id)
            .collect();
        let tag_map = self.session_first_tag(&pl_ids).await?;

        let mut groups: std::collections::BTreeMap<(Option<Uuid>, String), UninvoicedGroup> =
            std::collections::BTreeMap::new();
        for s in rows {
            let Some(end) = s.end_time else { continue };
            let (pid_key, tag_key) = match s.project_id {
                Some(pid) => (Some(pid), String::new()),
                None => (None, tag_map.get(&s.id).cloned().unwrap_or_default()),
            };
            let secs = (end - s.start_time).num_seconds().max(0);
            let g = groups
                .entry((pid_key, tag_key.clone()))
                .or_insert_with(|| UninvoicedGroup {
                    project_id: pid_key,
                    tag: tag_key.clone(),
                    session_count: 0,
                    seconds: 0,
                    amount_minor: 0,
                    currency: s.currency.clone(),
                });
            g.session_count += 1;
            g.seconds += secs;
            g.amount_minor += secs * s.rate_cents / 3600;
        }
        Ok(groups.into_values().collect())
    }

    async fn commit_inner(
        &self,
        book: Book,
        party: Party,
        invoice: Invoice,
        session_ids: Vec<Uuid>,
    ) -> Result<u64, FinanceError> {
        // Insert-if-missing book + party — the first committed
        // invoice in a fresh finance.sqlite creates them.
        BookEntity::insert(book_to_active(&book))
            .on_conflict(OnConflict::column(BookColumn::Id).do_nothing().to_owned())
            .do_nothing()
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        PartyEntity::insert(party_to_active(&party))
            .on_conflict(OnConflict::column(PartyColumn::Id).do_nothing().to_owned())
            .do_nothing()
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        // Typed duplicate-number guard ahead of the unique index.
        if !invoice.number.is_empty() {
            let dup = InvoiceEntity::find()
                .filter(InvoiceColumn::Number.eq(invoice.number.clone()))
                .one(&self.finance)
                .await
                .map_err(Self::err)?;
            if dup.is_some() {
                return Err(Self::backend(format!(
                    "invoice number `{}` already exists",
                    invoice.number
                )));
            }
        }
        InvoiceEntity::insert(invoice_to_active(&invoice))
            .exec(&self.finance)
            .await
            .map_err(Self::err)?;
        if session_ids.is_empty() {
            return Ok(0);
        }
        let stamped = WorkSessionEntity::update_many()
            .col_expr(WorkSessionColumn::InvoiceId, Expr::value(invoice.id))
            .col_expr(WorkSessionColumn::UpdatedAt, Expr::value(Utc::now()))
            .filter(WorkSessionColumn::Id.is_in(session_ids))
            .exec(&self.timer)
            .await
            .map_err(Self::err)?;
        Ok(stamped.rows_affected)
    }

    async fn void_inner(&self, id: Uuid) -> Result<u64, FinanceError> {
        let mut inv = self.get_inner(id).await?;
        if inv.amount_paid_minor > 0 {
            return Err(Self::backend(
                "invoice has payments against it; issue a credit note instead",
            ));
        }
        inv.status = InvoiceStatus::Cancelled;
        inv.updated_at = Utc::now();
        let mut active = invoice_to_active(&inv);
        active.id = sea_orm::ActiveValue::Unchanged(inv.id);
        active.update(&self.finance).await.map_err(Self::err)?;
        // Un-stamp so the hours become re-billable.
        let cleared = WorkSessionEntity::update_many()
            .col_expr(
                WorkSessionColumn::InvoiceId,
                Expr::value(Option::<Uuid>::None),
            )
            .col_expr(WorkSessionColumn::UpdatedAt, Expr::value(Utc::now()))
            .filter(WorkSessionColumn::InvoiceId.eq(id))
            .exec(&self.timer)
            .await
            .map_err(Self::err)?;
        Ok(cleared.rows_affected)
    }
}

impl Invoicing for FinanceBackend {
    fn generate_invoice(&self, req: GenerateInvoice) -> Result<Invoice, FinanceError> {
        self.runtime.block_on(self.generate_inner(req))
    }
    fn list_invoices(&self) -> Result<Vec<Invoice>, FinanceError> {
        self.runtime.block_on(self.list_inner())
    }
    fn get_invoice(&self, id: Uuid) -> Result<Invoice, FinanceError> {
        self.runtime.block_on(self.get_inner(id))
    }
    fn delete_invoice(&self, id: Uuid) -> Result<(), FinanceError> {
        self.runtime.block_on(self.delete_inner(id))
    }
    fn record_invoice_payment(
        &self,
        id: Uuid,
        amount_minor: i64,
        date: String,
    ) -> Result<Invoice, FinanceError> {
        self.runtime
            .block_on(self.pay_inner(id, amount_minor, date))
    }
    fn uninvoiced(&self) -> Result<Vec<UninvoicedGroup>, FinanceError> {
        self.runtime.block_on(self.uninvoiced_inner())
    }
    fn mark_sent(&self, id: Uuid) -> Result<Invoice, FinanceError> {
        self.runtime.block_on(self.mark_sent_inner(id))
    }

    // Deferred (ledger / credits / recurring) — not in the MVP.
    fn void_with_credit(&self, _id: Uuid, _reason: String) -> Result<Uuid, FinanceError> {
        Err(Self::backend("void_with_credit not implemented yet"))
    }
    fn record_payment(&self, _payload: RecordPayment) -> Result<Uuid, FinanceError> {
        Err(Self::backend(
            "record_payment (allocations) not implemented yet",
        ))
    }
    fn refund_payment(
        &self,
        _id: Uuid,
        _amount_minor: i64,
        _reason: String,
    ) -> Result<(), FinanceError> {
        Err(Self::backend("refund_payment not implemented yet"))
    }
    fn run_schedule_once(&self, _id: Uuid) -> Result<Option<Uuid>, FinanceError> {
        Err(Self::backend("run_schedule_once not implemented yet"))
    }
    fn commit_invoice(
        &self,
        book: Book,
        party: Party,
        invoice: Invoice,
        session_ids: Vec<Uuid>,
    ) -> Result<u64, FinanceError> {
        self.runtime
            .block_on(self.commit_inner(book, party, invoice, session_ids))
    }
    fn void_invoice(&self, id: Uuid) -> Result<u64, FinanceError> {
        self.runtime.block_on(self.void_inner(id))
    }
}

// ── ISO date helper ────────────────────────────────────────────────

fn parse_day(s: &str) -> Option<chrono::DateTime<Utc>> {
    let d = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?))
}

// ── Model ⇄ proto converters (architect Model has identical fields) ─

fn model_to_book(m: BookModel) -> Book {
    Book {
        id: m.id,
        name: m.name,
        kind: m.kind,
        base_currency: m.base_currency,
        settings_json: m.settings_json,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub fn book_to_active(b: &Book) -> BookActive {
    BookActive {
        id: Set(b.id),
        name: Set(b.name.clone()),
        kind: Set(b.kind),
        base_currency: Set(b.base_currency.clone()),
        settings_json: Set(b.settings_json.clone()),
        created_at: Set(b.created_at),
        updated_at: Set(b.updated_at),
    }
}

fn model_to_party(m: PartyModel) -> Party {
    Party {
        id: m.id,
        book_id: m.book_id,
        kind: m.kind,
        display_name: m.display_name,
        legal_name: m.legal_name,
        email: m.email,
        phone: m.phone,
        address: m.address,
        tax_id: m.tax_id,
        default_currency: m.default_currency,
        default_net_days: m.default_net_days,
        default_rate_minor_per_hour: m.default_rate_minor_per_hour,
        notes: m.notes,
        is_archived: m.is_archived,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub fn party_to_active(p: &Party) -> PartyActive {
    PartyActive {
        id: Set(p.id),
        book_id: Set(p.book_id),
        kind: Set(p.kind),
        display_name: Set(p.display_name.clone()),
        legal_name: Set(p.legal_name.clone()),
        email: Set(p.email.clone()),
        phone: Set(p.phone.clone()),
        address: Set(p.address.clone()),
        tax_id: Set(p.tax_id.clone()),
        default_currency: Set(p.default_currency.clone()),
        default_net_days: Set(p.default_net_days),
        default_rate_minor_per_hour: Set(p.default_rate_minor_per_hour),
        notes: Set(p.notes.clone()),
        is_archived: Set(p.is_archived),
        created_at: Set(p.created_at),
        updated_at: Set(p.updated_at),
    }
}

fn model_to_invoice(m: InvoiceModel) -> Invoice {
    Invoice {
        id: m.id,
        book_id: m.book_id,
        party_id: m.party_id,
        kind: m.kind,
        number: m.number,
        status: m.status,
        issue_date: m.issue_date,
        due_date: m.due_date,
        currency: m.currency,
        exchange_rate_micro: m.exchange_rate_micro,
        line_items: m.line_items,
        invoice_taxes: m.invoice_taxes,
        uses_inclusive_taxes: m.uses_inclusive_taxes,
        subtotal_minor: m.subtotal_minor,
        tax_total_minor: m.tax_total_minor,
        total_minor: m.total_minor,
        amount_paid_minor: m.amount_paid_minor,
        balance_minor: m.balance_minor,
        notes_public: m.notes_public,
        notes_private: m.notes_private,
        terms: m.terms,
        footer: m.footer,
        locked: m.locked,
        posted_at: m.posted_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

/// Marshal a [`finance_proto::invoice::Invoice`] into its
/// SeaORM active model. Used internally by the billing
/// pipeline and re-exported for external callers (the CLI
/// invoice-render flow) that need to insert an
/// already-built draft straight into `finance.sqlite`
/// without going through the full billing service.
pub fn invoice_to_active(i: &Invoice) -> InvoiceActive {
    InvoiceActive {
        id: Set(i.id),
        book_id: Set(i.book_id),
        party_id: Set(i.party_id),
        kind: Set(i.kind),
        number: Set(i.number.clone()),
        status: Set(i.status),
        issue_date: Set(i.issue_date.clone()),
        due_date: Set(i.due_date.clone()),
        currency: Set(i.currency.clone()),
        exchange_rate_micro: Set(i.exchange_rate_micro),
        line_items: Set(i.line_items.clone()),
        invoice_taxes: Set(i.invoice_taxes.clone()),
        uses_inclusive_taxes: Set(i.uses_inclusive_taxes),
        subtotal_minor: Set(i.subtotal_minor),
        tax_total_minor: Set(i.tax_total_minor),
        total_minor: Set(i.total_minor),
        amount_paid_minor: Set(i.amount_paid_minor),
        balance_minor: Set(i.balance_minor),
        notes_public: Set(i.notes_public.clone()),
        notes_private: Set(i.notes_private.clone()),
        terms: Set(i.terms.clone()),
        footer: Set(i.footer.clone()),
        locked: Set(i.locked),
        posted_at: Set(i.posted_at),
        created_at: Set(i.created_at),
        updated_at: Set(i.updated_at),
    }
}
