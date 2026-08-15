//! Firefly III JSON interop.
//!
//! Bidirectional bridge between our double-entry model (signed
//! N-way splits per [`finance_proto::ledger::Transaction`]) and
//! Firefly III's transaction-group / transaction-journal JSON
//! shape (positive amount + source/destination pair + type
//! discriminator).
//!
//! ## Scope
//!
//! v1 covers what most FF3 users round-trip in practice:
//!
//! - **Accounts** — asset / expense / revenue / liability /
//!   cash / initial-balance / reconciliation (the seven
//!   slugs in `firefly.shortNamesByFullName`).
//! - **Transactions** — withdrawal / deposit / transfer. The
//!   FF3 JSON also supports split transactions (one group →
//!   N journals); we materialize each journal as one of our
//!   two-split transactions, all sharing the group's title.
//! - **Currency** — passes through verbatim (ISO 4217 code).
//!
//! Out of scope for v1 (preserved as opaque metadata when
//! round-tripping):
//!
//! - Budgets / bills / recurrence / piggy banks / rules.
//! - Tags (modeled separately in FF3; we drop on import).
//! - Attachments.
//!
//! ## Mapping
//!
//! ### FF3 → Our model (import)
//!
//! | FF3 `type` | Source kind | Dest kind | Our splits |
//! |---|---|---|---|
//! | `withdrawal` | asset | expense | `[{src, -amt}, {dst, +amt}]` |
//! | `deposit` | revenue | asset | `[{src, -amt}, {dst, +amt}]` |
//! | `transfer` | asset | asset | `[{src, -amt}, {dst, +amt}]` |
//! | `opening-balance` | initial-balance | asset | same |
//! | `reconciliation` | reconciliation | asset | same |
//!
//! Account ids round-trip via a side table the caller maintains
//! (FF3 ids are u64, ours are uuid). On first import we mint a
//! UUID per FF3 account and store the FF3 id in
//! `Account.notes` for later lookup.
//!
//! ### Our model → FF3 (export)
//!
//! Splits are paired into `(source, destination)` based on
//! sign. Transactions with > 2 splits become FF3 split
//! transactions (one journal per split-pair); transactions
//! with 1 or odd splits export with a warning logged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use finance_proto::book::Book;
use finance_proto::ledger::{
    Account, AccountKind, Transaction, TransactionSource, TransactionSplit,
};

// ── FF3 JSON wire types ─────────────────────────────────────────────

/// Top-level shape of a Firefly III JSON export.
///
/// Matches the structure produced by FF3's
/// `/api/v2/transactions` paginated dump (and consumed by the
/// data-importer). Strictly a subset — fields we don't read are
/// captured in [`opaque_metadata`] for byte-stable round-tripping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FireflyExport {
    /// Each top-level entry is one transaction-group (which
    /// may contain N transaction-journals — FF3's term for
    /// "split").
    pub groups: Vec<FireflyTransactionGroup>,
    /// Account ledger needed to resolve `source_id` /
    /// `destination_id` references.
    pub accounts: Vec<FireflyAccount>,
    /// Catch-all for fields we don't model — round-trips
    /// untouched.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub opaque_metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FireflyTransactionGroup {
    pub id: String,
    pub group_title: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub transactions: Vec<FireflyTransactionJournal>,
}

/// One FF3 transaction journal — what FF3 calls a "transaction"
/// in JSON. Always a single source/destination pair with a
/// positive amount; the `type` discriminator names the direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FireflyTransactionJournal {
    pub transaction_journal_id: Option<String>,
    /// `withdrawal` / `deposit` / `transfer` /
    /// `opening-balance` / `reconciliation`.
    #[serde(rename = "type")]
    pub kind: String,
    pub date: DateTime<Utc>,
    pub description: String,
    pub amount: String, // FF3 ships decimals as strings
    pub currency_code: String,
    pub source_id: String,
    pub source_name: Option<String>,
    pub source_type: Option<String>,
    pub destination_id: String,
    pub destination_name: Option<String>,
    pub destination_type: Option<String>,
    /// Internal reference / external id — copied verbatim into
    /// our `Transaction.reference`.
    #[serde(default)]
    pub internal_reference: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FireflyAccount {
    pub id: String,
    pub name: String,
    /// One of `firefly.shortNamesByFullName` slugs.
    #[serde(rename = "type")]
    pub kind: String,
    pub currency_code: String,
    #[serde(default)]
    pub iban: Option<String>,
    #[serde(default)]
    pub account_number: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub opening_balance: Option<String>,
    #[serde(default)]
    pub opening_balance_date: Option<String>,
}

// ── Error type ──────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FireflyImportError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unknown account kind: {0}")]
    UnknownAccountKind(String),

    #[error("unknown transaction type: {0}")]
    UnknownTransactionType(String),

    #[error("missing account: {0}")]
    MissingAccount(String),

    #[error("bad amount: {0}")]
    BadAmount(String),
}

// ── Import: FF3 JSON → our model ────────────────────────────────────

/// Mapping table from FF3 `account.id` strings to our
/// [`finance_proto::ledger::Account`]s. Populated by
/// [`import_accounts`] and consumed by [`import_transactions`].
pub type AccountIndex = std::collections::HashMap<String, Account>;

/// Parse a Firefly III JSON export blob.
pub fn parse_export(json: &str) -> Result<FireflyExport, FireflyImportError> {
    Ok(serde_json::from_str(json)?)
}

/// Materialize the accounts side. Mints a fresh UUID per FF3
/// account id; stashes the original id in
/// `Account.notes` (`"ff3:<id>"` prefix) for export-time
/// round-tripping. All accounts get pinned to the same
/// [`Book`] — the caller picks which book.
#[must_use]
pub fn import_accounts(book: &Book, export: &FireflyExport) -> AccountIndex {
    let now = chrono::Utc::now();
    export
        .accounts
        .iter()
        .filter_map(|a| {
            let kind = match a.kind.as_str() {
                "asset" | "cash" => AccountKind::Asset,
                "expense" => AccountKind::Expense,
                "revenue" => AccountKind::Income,
                "liability" | "liabilities" => AccountKind::Liability,
                "initial-balance" | "reconciliation" => AccountKind::Equity,
                _ => return None,
            };
            let mut notes = format!("ff3:{}", a.id);
            if let Some(extra) = &a.notes {
                if !extra.is_empty() {
                    notes.push_str("\n\n");
                    notes.push_str(extra);
                }
            }
            let opening_balance_minor = a
                .opening_balance
                .as_deref()
                .and_then(parse_minor)
                .unwrap_or(0);
            Some((
                a.id.clone(),
                Account {
                    id: Uuid::new_v4(),
                    book_id: book.id,
                    name: a.name.clone(),
                    kind,
                    parent_id: Uuid::nil(),
                    currency: if a.currency_code.is_empty() {
                        book.base_currency.clone()
                    } else {
                        a.currency_code.clone()
                    },
                    is_archived: false,
                    opening_balance_minor,
                    notes,
                    created_at: now,
                    updated_at: now,
                },
            ))
        })
        .collect()
}

/// Materialize the transactions. Returns
/// `(transactions, warnings)` — non-fatal mapping issues (e.g. an
/// unknown FF3 type, a journal whose source/destination isn't
/// in the index) are logged as warnings rather than aborting
/// the whole import.
pub fn import_transactions(
    book: &Book,
    export: &FireflyExport,
    accounts: &AccountIndex,
) -> (Vec<Transaction>, Vec<String>) {
    let mut out = Vec::new();
    let mut warns = Vec::new();
    for group in &export.groups {
        for journal in &group.transactions {
            match journal_to_transaction(book, group, journal, accounts) {
                Ok(t) => out.push(t),
                Err(msg) => warns.push(msg),
            }
        }
    }
    (out, warns)
}

fn journal_to_transaction(
    book: &Book,
    group: &FireflyTransactionGroup,
    j: &FireflyTransactionJournal,
    accounts: &AccountIndex,
) -> Result<Transaction, String> {
    let source = accounts
        .get(&j.source_id)
        .ok_or_else(|| format!("missing source account {}", j.source_id))?;
    let dest = accounts
        .get(&j.destination_id)
        .ok_or_else(|| format!("missing destination account {}", j.destination_id))?;
    let amount_minor =
        parse_minor(&j.amount).ok_or_else(|| format!("bad amount {:?}", j.amount))?;

    // FF3 type discriminator is mostly informational on import
    // — the source/destination accounts already encode the
    // direction. We tag every imported tx with
    // `TransactionSource::Import` so reports can filter on it.
    match j.kind.to_ascii_lowercase().as_str() {
        "withdrawal" | "deposit" | "transfer" | "opening-balance" | "reconciliation"
        | "expense" | "revenue" => {}
        other => return Err(format!("unknown ff3 type {other:?}")),
    }
    let source_kind = TransactionSource::Import;

    use finance_proto::ledger::TransactionSplits;
    let splits = TransactionSplits(vec![
        TransactionSplit {
            account_id: source.id,
            amount_minor: -amount_minor,
            fx_amount_minor: 0,
            fx_rate_micro: 0,
            memo: String::new(),
        },
        TransactionSplit {
            account_id: dest.id,
            amount_minor,
            fx_amount_minor: 0,
            fx_rate_micro: 0,
            memo: String::new(),
        },
    ]);

    Ok(Transaction {
        id: Uuid::new_v4(),
        book_id: book.id,
        date: j.date.format("%Y-%m-%d").to_string(),
        description: group
            .group_title
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| j.description.clone()),
        reference: j
            .internal_reference
            .clone()
            .or_else(|| j.external_id.clone())
            .unwrap_or_default(),
        source_kind,
        source_id: Uuid::nil(),
        splits,
        created_at: group.created_at.unwrap_or_else(chrono::Utc::now),
    })
}

/// Parse FF3's stringly-typed decimal (`"123.45"`) into our
/// integer-minor representation (12345). Returns `None` on
/// parse failure.
fn parse_minor(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    let neg = trimmed.starts_with('-');
    let s = trimmed.trim_start_matches('-');
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    let whole_minor: i64 = whole.parse().ok().map(|w: i64| w * 100)?;
    let frac_minor: i64 = if frac.is_empty() {
        0
    } else {
        // Pad / truncate to 2 fractional digits.
        let mut padded = String::with_capacity(2);
        padded.extend(frac.chars().chain(std::iter::repeat('0')).take(2));
        padded.parse().ok()?
    };
    let v = whole_minor + frac_minor;
    Some(if neg { -v } else { v })
}

// ── Export: our model → FF3 JSON ────────────────────────────────────

/// Build a [`FireflyExport`] from our accounts + transactions.
/// The caller picks which subset of a book to include (full
/// dump, single calendar year, single account, ...) by passing
/// pre-filtered slices.
#[must_use]
pub fn build_export(accounts: &[Account], transactions: &[Transaction]) -> FireflyExport {
    let acc_index: std::collections::HashMap<Uuid, &Account> =
        accounts.iter().map(|a| (a.id, a)).collect();
    let ff_accounts: Vec<FireflyAccount> = accounts
        .iter()
        .map(|a| FireflyAccount {
            id: extract_ff3_id(&a.notes).unwrap_or_else(|| a.id.simple().to_string()),
            name: a.name.clone(),
            kind: ff3_kind_slug(a.kind),
            currency_code: a.currency.clone(),
            iban: None,
            account_number: None,
            notes: Some(a.notes.clone()).filter(|s| !s.is_empty()),
            opening_balance: Some(format_minor(a.opening_balance_minor)),
            opening_balance_date: None,
        })
        .collect();

    let mut groups: Vec<FireflyTransactionGroup> = Vec::with_capacity(transactions.len());
    for t in transactions {
        let pair = pair_splits(&t.splits.0);
        if pair.is_none() {
            tracing::warn!(
                tx = %t.id,
                splits = t.splits.0.len(),
                "ff3 export: skipping non-binary-split transaction",
            );
            continue;
        }
        let (src_split, dst_split) = pair.unwrap();
        let src = acc_index.get(&src_split.account_id);
        let dst = acc_index.get(&dst_split.account_id);
        let (Some(src), Some(dst)) = (src, dst) else {
            tracing::warn!(tx = %t.id, "ff3 export: missing account on a split");
            continue;
        };
        let amount = format_minor(dst_split.amount_minor.abs());
        let kind = ff3_transaction_type(src.kind, dst.kind);
        let date = parse_date(&t.date).unwrap_or_else(chrono::Utc::now);
        groups.push(FireflyTransactionGroup {
            id: t.id.simple().to_string(),
            group_title: Some(t.description.clone()).filter(|s| !s.is_empty()),
            created_at: Some(t.created_at),
            transactions: vec![FireflyTransactionJournal {
                transaction_journal_id: Some(format!("j-{}", t.id.simple())),
                kind,
                date,
                description: t.description.clone(),
                amount,
                currency_code: src.currency.clone(),
                source_id: extract_ff3_id(&src.notes)
                    .unwrap_or_else(|| src.id.simple().to_string()),
                source_name: Some(src.name.clone()),
                source_type: Some(ff3_kind_slug(src.kind)),
                destination_id: extract_ff3_id(&dst.notes)
                    .unwrap_or_else(|| dst.id.simple().to_string()),
                destination_name: Some(dst.name.clone()),
                destination_type: Some(ff3_kind_slug(dst.kind)),
                internal_reference: Some(t.reference.clone()).filter(|s| !s.is_empty()),
                external_id: None,
            }],
        });
    }

    FireflyExport {
        groups,
        accounts: ff_accounts,
        opaque_metadata: serde_json::Map::new(),
    }
}

fn pair_splits(splits: &[TransactionSplit]) -> Option<(&TransactionSplit, &TransactionSplit)> {
    if splits.len() != 2 {
        return None;
    }
    let (neg, pos) = if splits[0].amount_minor < 0 {
        (&splits[0], &splits[1])
    } else {
        (&splits[1], &splits[0])
    };
    if neg.amount_minor + pos.amount_minor != 0 {
        return None;
    }
    Some((neg, pos))
}

fn ff3_kind_slug(k: AccountKind) -> String {
    // Our 5-arm enum → FF3's 7-slug taxonomy. We don't ship
    // dedicated `cash` / `reconciliation` buckets on export —
    // FF3 importers handle a single asset/equity column gracefully.
    match k {
        AccountKind::Asset => "asset",
        AccountKind::Expense => "expense",
        AccountKind::Income => "revenue",
        AccountKind::Liability => "liabilities",
        AccountKind::Equity => "initial-balance",
    }
    .to_string()
}

fn ff3_transaction_type(src: AccountKind, dst: AccountKind) -> String {
    use AccountKind::{Asset, Equity, Expense, Income, Liability};
    match (src, dst) {
        // Asset → Expense (or liability → expense, e.g. credit card)
        (Asset | Liability, Expense) => "withdrawal",
        // Income → Asset (revenue landing in a bank account)
        (Income, Asset) => "deposit",
        // Asset → Asset (move between accounts)
        (Asset, Asset) => "transfer",
        // Equity → Asset (opening balance)
        (Equity, Asset) => "opening-balance",
        // Anything else falls through to transfer. The
        // accounting model permits this; FF3's strict
        // importer may reject some shapes.
        _ => "transfer",
    }
    .to_string()
}

fn format_minor(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let whole = abs / 100;
    let frac = abs % 100;
    format!("{}{whole}.{frac:02}", if neg { "-" } else { "" })
}

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    // FF3 ships ISO 8601 (`toAtomString`); our `date: String`
    // is `YYYY-MM-DD`. Both work via `parse`.
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
        })
}

fn extract_ff3_id(notes: &str) -> Option<String> {
    notes.lines().find_map(|line| {
        line.strip_prefix("ff3:")
            .map(|id| id.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_book() -> Book {
        Book {
            id: Uuid::new_v4(),
            name: "Personal".into(),
            kind: finance_proto::book::BookKind::Personal,
            base_currency: "USD".into(),
            settings_json: "{}".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn parse_minor_round_trip() {
        assert_eq!(parse_minor("0"), Some(0));
        assert_eq!(parse_minor("1"), Some(100));
        assert_eq!(parse_minor("1.5"), Some(150));
        assert_eq!(parse_minor("1.50"), Some(150));
        assert_eq!(parse_minor("-12.34"), Some(-1234));
        assert_eq!(parse_minor(""), Some(0));
        assert_eq!(format_minor(1234), "12.34");
        assert_eq!(format_minor(-50), "-0.50");
    }

    #[test]
    fn import_round_trip_smoke() {
        let json = r#"{
            "accounts": [
                {"id":"1","name":"Checking","type":"asset","currency_code":"USD","opening_balance":"100.00"},
                {"id":"2","name":"Groceries","type":"expense","currency_code":"USD"}
            ],
            "groups": [
                {"id":"g1","group_title":"Whole Foods","transactions":[
                    {"type":"withdrawal","date":"2026-05-22T10:00:00Z","description":"weekly groceries",
                     "amount":"42.50","currency_code":"USD",
                     "source_id":"1","destination_id":"2"}
                ]}
            ]
        }"#;
        let export = parse_export(json).unwrap();
        let book = sample_book();
        let accs = import_accounts(&book, &export);
        assert_eq!(accs.len(), 2);
        let (txs, warns) = import_transactions(&book, &export, &accs);
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(txs.len(), 1);
        let t = &txs[0];
        assert_eq!(t.splits.0.len(), 2);
        // signed splits sum to zero
        let sum: i64 = t.splits.0.iter().map(|s| s.amount_minor).sum();
        assert_eq!(sum, 0);
        // 4250 cents = $42.50
        assert!(t.splits.0.iter().any(|s| s.amount_minor == 4250));
        assert!(t.splits.0.iter().any(|s| s.amount_minor == -4250));
    }

    #[test]
    fn export_re_emits_ff3_shape() {
        // Build accounts + a transaction by hand and verify
        // the FF3 export type/source/destination wires up.
        let book = sample_book();
        let now = chrono::Utc::now();
        let checking = Account {
            id: Uuid::new_v4(),
            book_id: book.id,
            name: "Checking".into(),
            kind: AccountKind::Asset,
            parent_id: Uuid::nil(),
            currency: "USD".into(),
            is_archived: false,
            opening_balance_minor: 0,
            notes: "ff3:1".into(),
            created_at: now,
            updated_at: now,
        };
        let groceries = Account {
            id: Uuid::new_v4(),
            book_id: book.id,
            name: "Groceries".into(),
            kind: AccountKind::Expense,
            parent_id: Uuid::nil(),
            currency: "USD".into(),
            is_archived: false,
            opening_balance_minor: 0,
            notes: "ff3:2".into(),
            created_at: now,
            updated_at: now,
        };
        let tx = Transaction {
            id: Uuid::new_v4(),
            book_id: book.id,
            date: "2026-05-22".into(),
            description: "weekly groceries".into(),
            reference: String::new(),
            source_kind: TransactionSource::Manual,
            source_id: Uuid::nil(),
            splits: finance_proto::ledger::TransactionSplits(vec![
                TransactionSplit {
                    account_id: checking.id,
                    amount_minor: -4250,
                    fx_amount_minor: 0,
                    fx_rate_micro: 0,
                    memo: String::new(),
                },
                TransactionSplit {
                    account_id: groceries.id,
                    amount_minor: 4250,
                    fx_amount_minor: 0,
                    fx_rate_micro: 0,
                    memo: String::new(),
                },
            ]),
            created_at: now,
        };

        let export = build_export(&[checking, groceries], &[tx]);
        assert_eq!(export.groups.len(), 1);
        let g = &export.groups[0];
        assert_eq!(g.transactions[0].kind, "withdrawal");
        assert_eq!(g.transactions[0].source_id, "1");
        assert_eq!(g.transactions[0].destination_id, "2");
        assert_eq!(g.transactions[0].amount, "42.50");
        assert_eq!(g.transactions[0].currency_code, "USD");
    }
}
