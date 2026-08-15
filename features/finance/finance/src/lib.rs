// Stylo (transitive: finance → pdf → fulgur → blitz → style)
// blows rustc's trait-solver recursion limit on its giant
// generic `Debug` impl whenever feature unification activates
// the right combination of cfgs. Bump for the crate so the
// `pdf` feature can pull fulgur without surfacing the issue.
#![recursion_limit = "1024"]
//! `finance` — facade for the finance feature.
//!
//! ## Architectural carve-out
//!
//! Per `AGENTS.md` rule #2, Loro is the source of truth for
//! domain data. **Finance is the documented exception.** A
//! double-entry ledger needs hard invariants — sum-of-splits =
//! zero per transaction, unique invoice numbers per book, FK
//! integrity across `transactions ↔ splits ↔ accounts` — that
//! CRDT last-writer-wins merges actively destroy. So:
//!
//! - **Authoritative store**: `finance-db` (SeaORM over SQLite).
//! - **CRDT crate**: intentionally absent. Multi-device sync, if
//!   ever needed, goes through SQLite logical replication or a
//!   server-mediated event log — not Loro.
//! - **Vault representation**: one-way export into the
//!   **Records** sub-vault (`Records/Finance/{book}/...`).
//!   Records is graph-isolated + LLM-blocked by design, which
//!   matches finance's privacy posture exactly. The exporter
//!   writes plain-markdown snapshots of invoices, statements,
//!   and tax summaries; user edits to those files are ignored
//!   (drift warned, never round-tripped back into the DB).
//!
//! ## Surfaces
//!
//! - `export` — render markdown + PDF snapshots into the vault's
//!   Records sub-vault.
//! - `service` (forthcoming) — facade implementations of the
//!   `finance-proto` capability traits backed by `finance-db`.
//!
//! ## Time → Invoice flow
//!
//! Billable invoices materialize from `time-proto::TimeEntry`
//! rows tagged against a project + party. The "generate invoice
//! from unbilled time" action snapshots the entries into an
//! invoice's `line_items_json` and marks them billed — mirroring
//! InvoiceNinja's task→invoice coupling.

#![cfg(not(target_arch = "wasm32"))]

pub mod billing;
pub mod firefly;
pub mod invoice_from_sessions;
pub mod ledger;
pub mod pdf_adapter;
pub mod reports;

// Conventional re-export so `reports.rs` can write
// `finance_db_entity::InvoiceColumn` without forcing every
// consumer to repeat the path.
pub use finance_db::entity as finance_db_entity;

// Concrete service modules land here. The proto + db crates are
// the shape; the facade wires them together. `LedgerService` is
// the first — the `finance-db`-backed impl of the `Ledger`
// capability trait (post_transaction + balances + history).
pub use billing::FinanceBackend;
pub use ledger::LedgerService;

pub use finance_proto::*;
