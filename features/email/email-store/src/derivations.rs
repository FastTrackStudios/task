//! Derivation cache — storage for the triage pass. Pure rows;
//! the heuristics/LLM engines live in `email-product`.

use email_proto::{Derivation, DerivationKind};
use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

impl Store {
    /// Upsert one derivation row (replaces any prior version for
    /// the same `(message_id, kind)`).
    pub fn derivation_upsert(
        &mut self,
        message_id: &str,
        kind: DerivationKind,
        version: u32,
        payload: &str,
        now_ms: i64,
    ) -> Result<()> {
        self.conn_mut().execute(
            "INSERT INTO derivations (message_id, kind, version, payload, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id, kind) DO UPDATE SET
                 version = excluded.version,
                 payload = excluded.payload,
                 created_ms = excluded.created_ms",
            params![message_id, kind.as_str(), version, payload, now_ms],
        )?;
        Ok(())
    }

    /// Every current-version row for the given message-ids.
    /// Rows computed under an older version are omitted (they'll
    /// be recomputed by the pass).
    pub fn derivations_for(&self, ids: &[String], version: u32) -> Result<Vec<Derivation>> {
        let mut out = Vec::new();
        let mut stmt = self.conn().prepare(
            "SELECT kind, version, payload FROM derivations
             WHERE message_id = ?1 AND version = ?2",
        )?;
        for id in ids {
            let rows = stmt.query_map(params![id, version], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (kind, v, payload) = r?;
                let Some(kind) = DerivationKind::parse(&kind) else {
                    continue;
                };
                out.push(Derivation {
                    message_id: id.clone(),
                    kind,
                    version: v as u32,
                    payload,
                });
            }
        }
        Ok(out)
    }

    /// Of `candidates`, the message-ids that do NOT yet have a
    /// current-version `kind` row — the triage pass's work queue.
    /// Preserves candidate order (newest-first in, newest-first
    /// out).
    pub fn derivations_missing(
        &self,
        candidates: &[String],
        kind: DerivationKind,
        version: u32,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT 1 FROM derivations
             WHERE message_id = ?1 AND kind = ?2 AND version = ?3",
        )?;
        let mut missing = Vec::new();
        for id in candidates {
            let mut rows = stmt.query(params![id, kind.as_str(), version])?;
            if rows.next()?.is_none() {
                missing.push(id.clone());
            }
        }
        Ok(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_query_and_version_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();

        store
            .derivation_upsert("<a>", DerivationKind::Urgency, 1, "2", 100)
            .unwrap();
        store
            .derivation_upsert("<a>", DerivationKind::Tags, 1, "action-needed,calendar", 100)
            .unwrap();

        let ids = vec!["<a>".to_string(), "<b>".to_string()];
        let rows = store.derivations_for(&ids, 1).unwrap();
        assert_eq!(rows.len(), 2);
        let urgency = rows
            .iter()
            .find(|d| d.kind == DerivationKind::Urgency)
            .unwrap();
        assert_eq!(urgency.urgency(), Some(2));
        let tags = rows.iter().find(|d| d.kind == DerivationKind::Tags).unwrap();
        assert_eq!(tags.tags(), vec!["action-needed", "calendar"]);

        // <b> has nothing; <a> is done for v1.
        let missing = store
            .derivations_missing(&ids, DerivationKind::Urgency, 1)
            .unwrap();
        assert_eq!(missing, vec!["<b>".to_string()]);

        // Version bump invalidates: v2 reads see nothing, and the
        // pass sees <a> as missing again.
        assert!(store.derivations_for(&ids, 2).unwrap().is_empty());
        let missing_v2 = store
            .derivations_missing(&ids, DerivationKind::Urgency, 2)
            .unwrap();
        assert_eq!(missing_v2.len(), 2);

        // Recompute under v2 replaces the row.
        store
            .derivation_upsert("<a>", DerivationKind::Urgency, 2, "3", 200)
            .unwrap();
        let rows = store.derivations_for(&ids, 2).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].urgency(), Some(3));
    }
}
