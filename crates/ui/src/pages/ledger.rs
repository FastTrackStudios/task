//! `/ledger` — read-only double-entry ledger over the finance
//! `Ledger` service. Two sections per selected org:
//!
//! 1. **Accounts** — every account with its current balance, grouped
//!    by [`AccountKind`].
//! 2. **Transactions** — the recent journal entries (date,
//!    description, each split's account + debit/credit amount).
//!
//! Money renders from minor units (i64 cents) the same way
//! [`crate::pages::invoices`] does.

use crate::format::money;
use std::collections::HashMap;

use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use finance_proto::AccountBalance;
use finance_proto::ledger::{Account, AccountKind, Transaction};

use crate::orgs::{OrgMeta, OrgSelection};

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
    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));

    // One org at a time keeps the double-entry view coherent (balances
    // are per-book). Use the first selected slug.
    let slug = use_memo(move || slugs().first().cloned());

    // Keep the fetch `Result` intact (no in-closure `unwrap_or_default`)
    // so a failed book read surfaces an error + retry instead of a
    // silently-blank ledger.
    let mut accounts = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_ledger_accounts(&s).await,
            None => Ok(Vec::new()),
        }
    });
    let mut transactions = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_ledger_transactions(&s).await,
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
                crate::states::ErrorState {
                    title: "Couldn't load the ledger",
                    message: e,
                    on_retry: move |()| {
                        accounts.restart();
                        transactions.restart();
                    },
                }
            } else if loading {
                crate::states::LoadingState {}
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
