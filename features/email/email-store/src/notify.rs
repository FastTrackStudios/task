//! Notification state — first-sync baselining + alert-once
//! marks. Pure storage; the pass that feeds it lives in
//! `email-product`, and the notifications system consumes it via
//! the `EmailProduct` proto surface (`unnotified` /
//! `mark_notified`).

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

impl Store {
    /// Has this account's notification state been baselined?
    pub fn notify_is_baselined(&self) -> Result<bool> {
        let mut stmt = self
            .conn()
            .prepare("SELECT 1 FROM notify_meta WHERE key = 'baselined_ms'")?;
        Ok(stmt.query(params![])?.next()?.is_some())
    }

    /// First sight of the account: record every existing message
    /// as already-notified (silent), and stamp the baseline.
    /// Idempotent per message; the meta stamp flips
    /// [`Self::notify_is_baselined`].
    pub fn notify_baseline<'a, I>(&mut self, ids: I, now_ms: i64) -> Result<usize>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let tx = self.conn_mut().transaction()?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO notify_state (message_id, first_seen_ms, notified)
                 VALUES (?1, ?2, 1)",
            )?;
            for id in ids {
                inserted += stmt.execute(params![id, now_ms])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO notify_meta (key, value) VALUES ('baselined_ms', ?1)",
            params![now_ms.to_string()],
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Post-baseline pass: of `ids`, insert the never-seen ones
    /// with `notified = 0` — the one-and-only notification mark
    /// each message ever gets. At most `cap` new marks per call
    /// (the overflow is picked up by later passes). Returns the
    /// newly-marked ids.
    pub fn notify_observe<'a, I>(&mut self, ids: I, now_ms: i64, cap: usize) -> Result<Vec<String>>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let tx = self.conn_mut().transaction()?;
        let mut marked = Vec::new();
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO notify_state (message_id, first_seen_ms, notified)
                 VALUES (?1, ?2, 0)",
            )?;
            for id in ids {
                if marked.len() >= cap {
                    break;
                }
                if stmt.execute(params![id, now_ms])? > 0 {
                    marked.push(id.to_string());
                }
            }
        }
        tx.commit()?;
        Ok(marked)
    }

    /// Messages holding an undrained notification mark, newest
    /// first, up to `limit`.
    pub fn notify_unnotified(&self, limit: u32) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT message_id FROM notify_state
             WHERE notified = 0
             ORDER BY first_seen_ms DESC, message_id
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::from(limit)], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Drain marks: flip `notified` on for the given ids. Returns
    /// how many rows actually flipped (already-notified and
    /// unknown ids are no-ops).
    pub fn notify_mark<'a, I>(&mut self, ids: I) -> Result<u32>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let tx = self.conn_mut().transaction()?;
        let mut flipped = 0u32;
        {
            let mut stmt = tx.prepare(
                "UPDATE notify_state SET notified = 1 WHERE message_id = ?1 AND notified = 0",
            )?;
            for id in ids {
                flipped += stmt.execute(params![id])? as u32;
            }
        }
        tx.commit()?;
        Ok(flipped)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    #[test]
    fn baseline_is_silent_then_new_mail_marks_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();

        assert!(!store.notify_is_baselined().unwrap());

        // First sync: two existing messages baseline silently.
        store.notify_baseline(["<a>", "<b>"], 100).unwrap();
        assert!(store.notify_is_baselined().unwrap());
        assert!(store.notify_unnotified(10).unwrap().is_empty());

        // New message → exactly one mark.
        let marked = store.notify_observe(["<a>", "<b>", "<c>"], 200, 50).unwrap();
        assert_eq!(marked, vec!["<c>".to_string()]);
        assert_eq!(store.notify_unnotified(10).unwrap(), vec!["<c>".to_string()]);

        // Re-observing doesn't re-mark.
        assert!(store.notify_observe(["<c>"], 300, 50).unwrap().is_empty());

        // Drain; the mark never comes back.
        assert_eq!(store.notify_mark(["<c>"]).unwrap(), 1);
        assert!(store.notify_unnotified(10).unwrap().is_empty());
        assert_eq!(store.notify_mark(["<c>"]).unwrap(), 0);
        assert!(store.notify_observe(["<c>"], 400, 50).unwrap().is_empty());
    }

    #[test]
    fn observe_caps_new_marks_per_pass() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.notify_baseline(Vec::<&str>::new(), 0).unwrap();

        let ids: Vec<String> = (0..10).map(|i| format!("<m{i}>")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let marked = store.notify_observe(refs.iter().copied(), 100, 3).unwrap();
        assert_eq!(marked.len(), 3);
        // The overflow lands on the next pass.
        let marked2 = store.notify_observe(refs.iter().copied(), 200, 50).unwrap();
        assert_eq!(marked2.len(), 7);
    }
}
