//! Outbox — the staged-send state machine over the per-account
//! `outbox` table (schema v2). Pure storage + transition
//! enforcement; delivery, backoff policy, and event publishing
//! live in the `email-product` backend.
//!
//! The store is per-account, so rows carry no account column —
//! callers pass the account id purely so the returned
//! [`OutboxEntry`] payloads (a wire type) are fully populated.

use email_proto::{Draft, OutboxEntry, OutboxStatus};
use rusqlite::params;

use crate::error::{Result, StoreError};
use crate::store::Store;

/// Delivery attempts before an entry stops being auto-retried
/// and needs an explicit re-approve.
pub const OUTBOX_MAX_RETRIES: u32 = 5;

impl Store {
    /// Stage `draft` as a new `PendingApproval` entry.
    pub fn outbox_submit(
        &mut self,
        account: &str,
        draft: &Draft,
        origin: &str,
        now_ms: i64,
    ) -> Result<OutboxEntry> {
        let draft_json = serde_json::to_string(draft)?;
        self.conn_mut().execute(
            "INSERT INTO outbox (status, draft_json, origin, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                OutboxStatus::PendingApproval.as_str(),
                draft_json,
                origin,
                now_ms
            ],
        )?;
        let id = self.conn().last_insert_rowid() as u64;
        self.outbox_get(account, id)?
            .ok_or(StoreError::OutboxNotFound(id))
    }

    /// Every entry, newest first (terminal ones included so the
    /// panel shows outcomes).
    pub fn outbox_list(&self, account: &str) -> Result<Vec<OutboxEntry>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, status, draft_json, origin, created_ms, updated_ms,
                    retries, next_attempt_ms, last_error, sent_message_id
             FROM outbox ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |row| row_to_parts(row))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(parts_to_entry(account, r?)?);
        }
        Ok(out)
    }

    /// One entry by id.
    pub fn outbox_get(&self, account: &str, id: u64) -> Result<Option<OutboxEntry>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, status, draft_json, origin, created_ms, updated_ms,
                    retries, next_attempt_ms, last_error, sent_message_id
             FROM outbox WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id as i64], |row| row_to_parts(row))?;
        match rows.next() {
            Some(r) => Ok(Some(parts_to_entry(account, r?)?)),
            None => Ok(None),
        }
    }

    /// `PendingApproval` / `Failed` → `Approved`. Re-approving a
    /// failed entry resets its retry budget.
    pub fn outbox_approve(&mut self, account: &str, id: u64, now_ms: i64) -> Result<OutboxEntry> {
        self.outbox_transition(
            account,
            id,
            "approve",
            &[OutboxStatus::PendingApproval, OutboxStatus::Failed],
            |tx| {
                tx.execute(
                    "UPDATE outbox
                     SET status = ?2, updated_ms = ?3, retries = 0,
                         next_attempt_ms = 0, last_error = NULL
                     WHERE id = ?1",
                    params![id as i64, OutboxStatus::Approved.as_str(), now_ms],
                )
            },
        )
    }

    /// Any pre-delivery state → `Cancelled`.
    pub fn outbox_cancel(&mut self, account: &str, id: u64, now_ms: i64) -> Result<OutboxEntry> {
        self.outbox_transition(
            account,
            id,
            "cancel",
            &[
                OutboxStatus::Draft,
                OutboxStatus::PendingApproval,
                OutboxStatus::Approved,
                OutboxStatus::Failed,
            ],
            |tx| {
                tx.execute(
                    "UPDATE outbox SET status = ?2, updated_ms = ?3 WHERE id = ?1",
                    params![id as i64, OutboxStatus::Cancelled.as_str(), now_ms],
                )
            },
        )
    }

    /// Claim up to `limit` due entries for delivery: `Approved`
    /// entries, plus `Failed` ones whose backoff has elapsed and
    /// whose retry budget isn't exhausted. Claimed entries flip
    /// to `Sending` atomically so a second poller pass can't
    /// double-deliver.
    pub fn outbox_claim_due(
        &mut self,
        account: &str,
        now_ms: i64,
        limit: u32,
    ) -> Result<Vec<OutboxEntry>> {
        let tx = self.conn_mut().transaction()?;
        let ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM outbox
                 WHERE (status = 'approved' AND next_attempt_ms <= ?1)
                    OR (status = 'failed' AND next_attempt_ms <= ?1 AND retries < ?2)
                 ORDER BY id ASC LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![now_ms, OUTBOX_MAX_RETRIES, i64::from(limit)],
                |row| row.get::<_, i64>(0),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE outbox SET status = 'sending', updated_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
        }
        tx.commit()?;

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(e) = self.outbox_get(account, id as u64)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// `Sending` → `Sent`, recording the sent copy's Message-ID.
    pub fn outbox_mark_sent(
        &mut self,
        account: &str,
        id: u64,
        message_id: &str,
        now_ms: i64,
    ) -> Result<OutboxEntry> {
        self.outbox_transition(account, id, "mark_sent", &[OutboxStatus::Sending], |tx| {
            tx.execute(
                "UPDATE outbox
                 SET status = 'sent', updated_ms = ?2, sent_message_id = ?3,
                     last_error = NULL
                 WHERE id = ?1",
                params![id as i64, now_ms, message_id],
            )
        })
    }

    /// `Sending` → `Failed`, bumping the retry counter and
    /// scheduling the next attempt at `next_attempt_ms` (backoff
    /// policy is the caller's).
    pub fn outbox_mark_failed(
        &mut self,
        account: &str,
        id: u64,
        error: &str,
        now_ms: i64,
        next_attempt_ms: i64,
    ) -> Result<OutboxEntry> {
        self.outbox_transition(account, id, "mark_failed", &[OutboxStatus::Sending], |tx| {
            tx.execute(
                "UPDATE outbox
                 SET status = 'failed', updated_ms = ?2, retries = retries + 1,
                     last_error = ?3, next_attempt_ms = ?4
                 WHERE id = ?1",
                params![id as i64, now_ms, error, next_attempt_ms],
            )
        })
    }

    /// Shared transition guard: load, check the current status is
    /// in `allowed`, apply `update` in a transaction, return the
    /// fresh row.
    fn outbox_transition(
        &mut self,
        account: &str,
        id: u64,
        op: &'static str,
        allowed: &[OutboxStatus],
        update: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<usize>,
    ) -> Result<OutboxEntry> {
        let tx = self.conn_mut().transaction()?;
        let status: Option<String> = {
            let mut stmt = tx.prepare("SELECT status FROM outbox WHERE id = ?1")?;
            let mut rows = stmt.query_map(params![id as i64], |row| row.get(0))?;
            rows.next().transpose()?
        };
        let Some(status) = status else {
            return Err(StoreError::OutboxNotFound(id));
        };
        let current = OutboxStatus::parse(&status)
            .ok_or_else(|| StoreError::Parse(format!("bad outbox status {status:?}")))?;
        if !allowed.contains(&current) {
            return Err(StoreError::OutboxTransition {
                id,
                from: current.as_str(),
                op,
            });
        }
        update(&tx)?;
        tx.commit()?;
        self.outbox_get(account, id)?
            .ok_or(StoreError::OutboxNotFound(id))
    }
}

type Parts = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
);

fn row_to_parts(row: &rusqlite::Row<'_>) -> rusqlite::Result<Parts> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn parts_to_entry(account: &str, p: Parts) -> Result<OutboxEntry> {
    let (
        id,
        status,
        draft_json,
        origin,
        created_ms,
        updated_ms,
        retries,
        next_attempt_ms,
        last_error,
        sent_message_id,
    ) = p;
    let status = OutboxStatus::parse(&status)
        .ok_or_else(|| StoreError::Parse(format!("bad outbox status {status:?}")))?;
    let draft: Draft = serde_json::from_str(&draft_json)?;
    Ok(OutboxEntry {
        id: id as u64,
        account: account.to_string(),
        status,
        draft,
        origin,
        created_ms,
        updated_ms,
        retries: retries as u32,
        next_attempt_ms,
        last_error,
        sent_message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::Addr;

    fn draft() -> Draft {
        Draft {
            from: Addr {
                name: None,
                email: "you@example.com".into(),
            },
            to: vec![Addr {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Staged".into(),
            body_text: "outbox body".into(),
            body_html: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        }
    }

    #[test]
    fn submit_approve_claim_sent_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();

        let e = store
            .outbox_submit("acct", &draft(), "user", 1_000)
            .unwrap();
        assert_eq!(e.status, OutboxStatus::PendingApproval);
        assert_eq!(e.account, "acct");
        assert_eq!(e.origin, "user");

        // Not claimable while pending.
        assert!(
            store
                .outbox_claim_due("acct", 2_000, 10)
                .unwrap()
                .is_empty()
        );

        let e = store.outbox_approve("acct", e.id, 2_000).unwrap();
        assert_eq!(e.status, OutboxStatus::Approved);

        let claimed = store.outbox_claim_due("acct", 3_000, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, OutboxStatus::Sending);
        // Second claim finds nothing — no double delivery.
        assert!(
            store
                .outbox_claim_due("acct", 3_000, 10)
                .unwrap()
                .is_empty()
        );

        let e = store
            .outbox_mark_sent("acct", claimed[0].id, "<m@x>", 4_000)
            .unwrap();
        assert_eq!(e.status, OutboxStatus::Sent);
        assert_eq!(e.sent_message_id.as_deref(), Some("<m@x>"));
    }

    #[test]
    fn failed_entries_back_off_then_exhaust() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let e = store.outbox_submit("acct", &draft(), "agent:x", 0).unwrap();
        store.outbox_approve("acct", e.id, 0).unwrap();

        for attempt in 0..OUTBOX_MAX_RETRIES {
            let claimed = store
                .outbox_claim_due("acct", i64::from(attempt) * 100_000, 10)
                .unwrap();
            assert_eq!(claimed.len(), 1, "attempt {attempt}");
            let f = store
                .outbox_mark_failed(
                    "acct",
                    e.id,
                    "smtp down",
                    i64::from(attempt) * 100_000,
                    i64::from(attempt + 1) * 100_000 - 1,
                )
                .unwrap();
            assert_eq!(f.status, OutboxStatus::Failed);
            assert_eq!(f.retries, attempt + 1);
        }

        // Budget exhausted — never claimed again, however late.
        assert!(
            store
                .outbox_claim_due("acct", i64::MAX, 10)
                .unwrap()
                .is_empty()
        );

        // Re-approval resets the budget.
        let e = store.outbox_approve("acct", e.id, 999).unwrap();
        assert_eq!(e.retries, 0);
        assert_eq!(store.outbox_claim_due("acct", 999, 10).unwrap().len(), 1);
    }

    #[test]
    fn cancel_gates_on_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let e = store.outbox_submit("acct", &draft(), "user", 0).unwrap();
        let e = store.outbox_cancel("acct", e.id, 1).unwrap();
        assert_eq!(e.status, OutboxStatus::Cancelled);
        // Cancelling again is an invalid transition.
        assert!(matches!(
            store.outbox_cancel("acct", e.id, 2),
            Err(StoreError::OutboxTransition { .. })
        ));
        // Unknown id.
        assert!(matches!(
            store.outbox_cancel("acct", 999, 2),
            Err(StoreError::OutboxNotFound(999))
        ));
    }
}
