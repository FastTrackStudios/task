//! `Store` — the SQLite-backed cache. One per account.
//!
//! Threading model: `Store` wraps a single `rusqlite::Connection`
//! and is `!Sync` by virtue of that. The sync engine owns one
//! `Store` and serializes writes through it; readers can either
//! share via `Arc<Mutex<Store>>` or open their own read-only
//! handles. Phase 1 takes the simple route.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use email_proto::{Addr, Envelope};
use mail_parser::MessageParser;
use rusqlite::{Connection, Transaction, params};

use crate::error::Result;
use crate::query::{SearchHit, StoredEnvelope};
use crate::schema::{SCHEMA_V1, SCHEMA_V2, SCHEMA_VERSION};
use crate::walker;

pub struct Store {
    pub account_root: PathBuf,
    conn: Connection,
}

impl Store {
    /// Open (or create) the index at `<account_root>/index.db`.
    /// The maildir tree itself isn't touched here — call
    /// [`Self::rebuild_from_disk`] to populate from scratch.
    pub fn open(account_root: impl Into<PathBuf>) -> Result<Self> {
        let account_root = account_root.into();
        std::fs::create_dir_all(&account_root)?;
        let db_path = account_root.join("index.db");
        let conn = Connection::open(&db_path)?;
        // Sensible pragmas for a single-writer mailbox cache:
        // WAL → readers don't block the sync engine's writes;
        // NORMAL synchronous → durable enough for a disposable
        // index; foreign_keys ON for the (eventual) thread
        // relationship.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch(SCHEMA_V2)?;
        let store = Self { account_root, conn };
        store.set_user_version(SCHEMA_VERSION)?;
        Ok(store)
    }

    fn set_user_version(&self, v: i64) -> Result<()> {
        self.conn.pragma_update(None, "user_version", v)?;
        Ok(())
    }

    /// Connection access for the product-layer modules in this
    /// crate (`outbox`, later `derivations` / `notify`).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Insert-or-replace an envelope. `path` / `content_hash` /
    /// body text are optional; the store happily holds
    /// server-only entries that aren't yet on disk.
    pub fn upsert_envelope(
        &mut self,
        env: &Envelope,
        path: Option<&str>,
        content_hash: Option<&str>,
        body_text: Option<&str>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::upsert_envelope_tx(&tx, env, path, content_hash, body_text)?;
        tx.commit()?;
        Ok(())
    }

    fn upsert_envelope_tx(
        tx: &Transaction<'_>,
        env: &Envelope,
        path: Option<&str>,
        content_hash: Option<&str>,
        body_text: Option<&str>,
    ) -> Result<()> {
        let from_addr = json_addr_list(&env.from);
        let to_addrs = json_addr_list(&env.to);
        let cc_addrs = json_addr_list(&env.cc);
        let flags = serde_json::to_string(&env.flags)?;
        let rowid = stable_rowid(&env.message_id);

        tx.execute(
            "INSERT INTO messages (
                message_id, folder, thread_id, subject, from_addr, to_addrs, cc_addrs,
                date_ms, flags, size, has_atts, snippet, path, content_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(message_id) DO UPDATE SET
                folder       = excluded.folder,
                thread_id    = excluded.thread_id,
                subject      = excluded.subject,
                from_addr    = excluded.from_addr,
                to_addrs     = excluded.to_addrs,
                cc_addrs     = excluded.cc_addrs,
                date_ms      = excluded.date_ms,
                flags        = excluded.flags,
                size         = excluded.size,
                has_atts     = excluded.has_atts,
                snippet      = COALESCE(excluded.snippet, messages.snippet),
                path         = COALESCE(excluded.path, messages.path),
                content_hash = COALESCE(excluded.content_hash, messages.content_hash)",
            params![
                env.message_id,
                env.folder,
                env.thread_id,
                env.subject,
                from_addr,
                to_addrs,
                cc_addrs,
                env.date_ms,
                flags,
                env.size as i64,
                i64::from(env.has_attachments),
                env.snippet,
                path,
                content_hash,
            ],
        )?;

        // FTS5 row — replace-then-insert so updates work.
        tx.execute("DELETE FROM messages_fts WHERE rowid = ?1", params![rowid])?;
        tx.execute(
            "INSERT INTO messages_fts (rowid, subject, from_addr, to_addrs, body_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                rowid,
                env.subject,
                from_addr_text(&env.from),
                to_addr_text(&env.to),
                body_text.unwrap_or(""),
            ],
        )?;

        Ok(())
    }

    /// Delete one message by message-id. Idempotent — removing
    /// an absent id is not an error.
    pub fn delete_message(&mut self, message_id: &str) -> Result<()> {
        let rowid = stable_rowid(message_id);
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE message_id = ?1",
            params![message_id],
        )?;
        tx.execute("DELETE FROM messages_fts WHERE rowid = ?1", params![rowid])?;
        tx.commit()?;
        Ok(())
    }

    /// Every message-id currently indexed under `folder`. Used
    /// by `email-sync` to compute diffs against the latest
    /// backend snapshot.
    pub fn known_message_ids(&self, folder: &str) -> Result<BTreeSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT message_id FROM messages WHERE folder = ?1")?;
        let rows = stmt.query_map(params![folder], |row| row.get::<_, String>(0))?;
        let mut out = BTreeSet::new();
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }

    /// Newest-first envelopes in one folder.
    pub fn query_envelopes(&self, folder: &str, limit: u32) -> Result<Vec<StoredEnvelope>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, folder, thread_id, subject, from_addr, to_addrs, cc_addrs,
                    date_ms, flags, size, has_atts, snippet, path, content_hash
             FROM messages
             WHERE folder = ?1
             ORDER BY date_ms DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![folder, i64::from(limit)], row_to_stored)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Per-folder unread + total counts.
    pub fn folder_counts(&self) -> Result<BTreeMap<String, (u32, u32)>> {
        let mut stmt = self.conn.prepare("SELECT folder, flags FROM messages")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for r in rows {
            let (folder, flags_json) = r?;
            let flags: Vec<String> = serde_json::from_str(&flags_json).unwrap_or_default();
            let seen = flags
                .iter()
                .any(|f| f == "S" || f == "\\Seen" || f == "Seen");
            let entry = out.entry(folder).or_insert((0, 0));
            entry.0 += 1;
            if !seen {
                entry.1 += 1;
            }
        }
        Ok(out)
    }

    /// FTS5 search. Matches `subject` / `from` / `to` / `body`.
    /// `query` is passed to FTS5 verbatim — callers should
    /// escape user input as needed.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        // Two-pass: FTS5 gives us (rank, rowid); then we resolve
        // each rowid to a full envelope. We don't store a
        // reverse `rowid → message_id` index since FTS5 hits
        // are typically small (top-N) — scanning the messages
        // table for a stable_rowid match is fast on the
        // realistic mailbox sizes.
        let mut stmt = self.conn.prepare(
            "SELECT rowid, rank
             FROM messages_fts
             WHERE messages_fts MATCH ?1
             ORDER BY rank ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, i64::from(limit)], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;

        let mut hits = Vec::new();
        for r in rows {
            let (rowid, rank) = r?;
            // Resolve rowid → envelope by scanning. Acceptable
            // for top-N hits; the clean fix is a covering
            // `fts_rowid` column on `messages` with an index
            // (tracked in the plan).
            let mut q = self.conn.prepare(
                "SELECT message_id, folder, thread_id, subject, from_addr, to_addrs,
                        cc_addrs, date_ms, flags, size, has_atts, snippet, path, content_hash
                 FROM messages",
            )?;
            let iter = q.query_map([], row_to_stored)?;
            for s in iter {
                let s = s?;
                if stable_rowid(&s.envelope.message_id) == rowid {
                    hits.push(SearchHit { rank, envelope: s });
                    break;
                }
            }
        }
        Ok(hits)
    }

    /// Re-run JWZ thread reconstruction against every indexed
    /// message and update `thread_id` per the algorithm. Walks
    /// the `messages` table twice (once to read, once to
    /// update); cheap on realistic mailbox sizes since the
    /// algorithm is O(N + edges) memory + ~O(N) lookups.
    ///
    /// Currently uses each message's `thread_id` field (already
    /// populated with In-Reply-To at upsert time) as the only
    /// reference signal. A future pass will store the full
    /// `References` header in a sidecar column for proper
    /// multi-hop chain resolution; the JWZ implementation
    /// already accepts the full reference list when we have it.
    pub fn rebuild_threads(&mut self) -> Result<usize> {
        // Pull all (message_id, in_reply_to) rows.
        let rows: Vec<(String, Option<String>)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT message_id, thread_id FROM messages")?;
            let iter = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            iter.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Translate to the borrowed ThreadInput shape JWZ
        // expects. References vec is owned; we hand out borrows.
        let inputs: Vec<crate::jwz::ThreadInput<'_>> = rows
            .iter()
            .map(|(mid, parent)| crate::jwz::ThreadInput {
                message_id: mid.as_str(),
                in_reply_to: parent.as_deref(),
                references: &[],
            })
            .collect();
        let assignments = crate::jwz::compute_threads(&inputs);

        let tx = self.conn.transaction()?;
        let mut updated = 0usize;
        {
            let mut stmt =
                tx.prepare("UPDATE messages SET thread_id = ?1 WHERE message_id = ?2")?;
            for a in &assignments {
                stmt.execute(params![a.thread_id, a.message_id])?;
                updated += 1;
            }
        }
        tx.commit()?;
        Ok(updated)
    }

    /// Rebuild the index from on-disk maildirs. Parses headers
    /// with mail-parser and re-indexes every message. Slow on
    /// big mailboxes — meant for the once-per-machine warmup
    /// path. Returns the count of indexed messages.
    pub fn rebuild_from_disk(&mut self) -> Result<usize> {
        let entries = walker::walk_account(&self.account_root);
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM messages", [])?;
        tx.execute("DELETE FROM messages_fts", [])?;

        let mut indexed = 0usize;
        for entry in entries {
            let bytes = match std::fs::read(&entry.abs_path) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(path = %entry.abs_path.display(), %err, "skip unreadable");
                    continue;
                }
            };
            let Some(env) = parse_envelope(&bytes, &entry.folder, &entry.flags) else {
                tracing::warn!(path = %entry.abs_path.display(), "parse failed");
                continue;
            };
            let body_text = body_text(&bytes);
            let rel = entry.rel_path.to_string_lossy().into_owned();
            let hash = content_hash_hex(&bytes);
            Self::upsert_envelope_tx(&tx, &env, Some(&rel), Some(&hash), body_text.as_deref())?;
            indexed += 1;
        }
        tx.commit()?;
        Ok(indexed)
    }
}

/// Stable rowid derived from message-id. `SQLite` rowids are i64;
/// we take the low 63 bits of `FxHash` and clear the sign bit so
/// the value is always positive (`SQLite` reserves negatives).
fn stable_rowid(message_id: &str) -> i64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(message_id.as_bytes());
    (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

fn json_addr_list(addrs: &[Addr]) -> String {
    serde_json::to_string(addrs).unwrap_or_else(|_| "[]".into())
}

fn from_addr_text(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(|a| match &a.name {
            Some(n) => format!("{n} {} {}", '<', a.email),
            None => a.email.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn to_addr_text(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(|a| a.email.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

fn row_to_stored(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEnvelope> {
    let message_id: String = row.get(0)?;
    let folder: String = row.get(1)?;
    let thread_id: Option<String> = row.get(2)?;
    let subject: String = row.get(3)?;
    let from_json: String = row.get(4)?;
    let to_json: String = row.get(5)?;
    let cc_json: String = row.get(6)?;
    let date_ms: i64 = row.get(7)?;
    let flags_json: String = row.get(8)?;
    let size: i64 = row.get(9)?;
    let has_atts: i64 = row.get(10)?;
    let snippet: Option<String> = row.get(11)?;
    let path: Option<String> = row.get(12)?;
    let content_hash: Option<String> = row.get(13)?;

    let from: Vec<Addr> = serde_json::from_str(&from_json).unwrap_or_default();
    let to: Vec<Addr> = serde_json::from_str(&to_json).unwrap_or_default();
    let cc: Vec<Addr> = serde_json::from_str(&cc_json).unwrap_or_default();
    let flags: Vec<String> = serde_json::from_str(&flags_json).unwrap_or_default();

    Ok(StoredEnvelope {
        envelope: Envelope {
            message_id,
            folder,
            thread_id,
            subject,
            from,
            to,
            cc,
            date_ms,
            flags,
            has_attachments: has_atts != 0,
            size: size as u64,
            snippet,
        },
        path,
        content_hash,
    })
}

fn parse_envelope(bytes: &[u8], folder: &str, flag_chars: &[char]) -> Option<Envelope> {
    let parsed = MessageParser::default().parse_headers(bytes)?;
    let subject = parsed.subject().unwrap_or("").to_string();
    let from = parsed
        .from()
        .map(|a| {
            a.iter()
                .map(|x| Addr {
                    name: x.name().map(std::string::ToString::to_string),
                    email: x.address().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let to = parsed
        .to()
        .map(|a| {
            a.iter()
                .map(|x| Addr {
                    name: x.name().map(std::string::ToString::to_string),
                    email: x.address().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let cc = parsed
        .cc()
        .map(|a| {
            a.iter()
                .map(|x| Addr {
                    name: x.name().map(std::string::ToString::to_string),
                    email: x.address().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let date_ms = parsed
        .date()
        .map_or(0, |d| d.to_timestamp().saturating_mul(1000));
    let message_id = parsed.message_id().map_or_else(
        || format!("<sha-{}@local.store>", content_hash_hex(bytes)),
        std::string::ToString::to_string,
    );
    let thread_id = parsed
        .in_reply_to()
        .as_text()
        .map(std::string::ToString::to_string);
    let has_attachments = parsed
        .headers()
        .iter()
        .any(|h| h.name().eq_ignore_ascii_case("Content-Disposition"));
    let flags: Vec<String> = flag_chars
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    Some(Envelope {
        message_id,
        folder: folder.to_string(),
        thread_id,
        subject,
        from,
        to,
        cc,
        date_ms,
        flags,
        has_attachments,
        size: bytes.len() as u64,
        snippet: None,
    })
}

fn body_text(bytes: &[u8]) -> Option<String> {
    let parsed = MessageParser::default().parse(bytes)?;
    parsed.body_text(0).map(std::borrow::Cow::into_owned)
}

fn content_hash_hex(bytes: &[u8]) -> String {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    format!("{:016x}", h.finish())
}

#[allow(dead_code)]
fn _path_used(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::Addr;

    fn env(msgid: &str, folder: &str, subject: &str, body_words: &str) -> Envelope {
        Envelope {
            message_id: msgid.into(),
            folder: folder.into(),
            thread_id: None,
            subject: subject.into(),
            from: vec![Addr {
                name: Some("Alice".into()),
                email: "alice@example.com".into(),
            }],
            to: vec![Addr {
                name: None,
                email: "you@example.com".into(),
            }],
            cc: vec![],
            date_ms: 1_700_000_000_000,
            flags: vec![],
            has_attachments: false,
            size: body_words.len() as u64,
            snippet: None,
        }
    }

    #[test]
    fn upsert_and_query_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let e = env(
            "<a@example.com>",
            "INBOX",
            "Hello world",
            "hello world from alice",
        );
        store
            .upsert_envelope(&e, None, None, Some("hello world from alice"))
            .unwrap();
        let got = store.query_envelopes("INBOX", 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].envelope.subject, "Hello world");
    }

    #[test]
    fn known_message_ids_returns_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store
            .upsert_envelope(&env("<a>", "INBOX", "x", ""), None, None, None)
            .unwrap();
        store
            .upsert_envelope(&env("<b>", "INBOX", "y", ""), None, None, None)
            .unwrap();
        let ids = store.known_message_ids("INBOX").unwrap();
        assert!(ids.contains("<a>"));
        assert!(ids.contains("<b>"));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn delete_removes_from_both_tables() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store
            .upsert_envelope(
                &env("<a>", "INBOX", "Find me", ""),
                None,
                None,
                Some("findbody"),
            )
            .unwrap();
        assert_eq!(store.query_envelopes("INBOX", 10).unwrap().len(), 1);
        store.delete_message("<a>").unwrap();
        assert_eq!(store.query_envelopes("INBOX", 10).unwrap().len(), 0);
        // FTS5 hit should be gone too.
        assert!(store.search("findbody", 10).unwrap().is_empty());
    }

    #[test]
    fn search_finds_body_match() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store
            .upsert_envelope(
                &env("<a>", "INBOX", "Status report", ""),
                None,
                None,
                Some("the cluster is healthy and the migration completed"),
            )
            .unwrap();
        store
            .upsert_envelope(
                &env("<b>", "INBOX", "Lunch", ""),
                None,
                None,
                Some("burrito wrapped in foil"),
            )
            .unwrap();
        let hits = store.search("migration", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].envelope.envelope.message_id, "<a>");
    }

    #[test]
    fn rebuild_threads_assigns_root_to_chain() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();

        let mut a = env("<root>", "INBOX", "Start", "");
        a.thread_id = None;
        let mut b = env("<r1>", "INBOX", "Re: Start", "");
        b.thread_id = Some("<root>".into());
        let mut c = env("<r2>", "INBOX", "Re: Re: Start", "");
        c.thread_id = Some("<r1>".into());
        let mut other = env("<x>", "INBOX", "Unrelated", "");
        other.thread_id = None;

        store.upsert_envelope(&a, None, None, None).unwrap();
        store.upsert_envelope(&b, None, None, None).unwrap();
        store.upsert_envelope(&c, None, None, None).unwrap();
        store.upsert_envelope(&other, None, None, None).unwrap();

        let n = store.rebuild_threads().unwrap();
        assert_eq!(n, 4);

        let envs = store.query_envelopes("INBOX", 10).unwrap();
        let map: std::collections::HashMap<_, _> = envs
            .iter()
            .map(|s| (s.envelope.message_id.clone(), s.envelope.thread_id.clone()))
            .collect();
        assert_eq!(map["<root>"], Some("<root>".into()));
        assert_eq!(map["<r1>"], Some("<root>".into()));
        assert_eq!(map["<r2>"], Some("<root>".into()));
        assert_eq!(map["<x>"], Some("<x>".into()));
    }

    #[test]
    fn folder_counts_track_seen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut e = env("<a>", "INBOX", "x", "");
        store.upsert_envelope(&e, None, None, None).unwrap();
        e.flags = vec!["S".into()];
        let mut e2 = env("<b>", "INBOX", "y", "");
        e2.flags = vec!["S".into()];
        store.upsert_envelope(&e, None, None, None).unwrap();
        store.upsert_envelope(&e2, None, None, None).unwrap();
        let counts = store.folder_counts().unwrap();
        let (total, unread) = counts.get("INBOX").copied().unwrap_or((0, 0));
        assert_eq!(total, 2);
        // Both have `S` so unread = 0.
        assert_eq!(unread, 0);
    }

    #[test]
    fn rebuild_from_disk_indexes_walker_output() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        std::fs::write(
            dir.path().join("new/1.M.host"),
            b"Message-ID: <r1@example.com>\r\n\
              From: a@example.com\r\n\
              To: you@example.com\r\n\
              Subject: Rebuild me\r\n\
              Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
              \r\n\
              the body has the word groundhog\r\n",
        )
        .unwrap();

        let mut store = Store::open(dir.path()).unwrap();
        let n = store.rebuild_from_disk().unwrap();
        assert_eq!(n, 1);
        let envs = store.query_envelopes("INBOX", 10).unwrap();
        assert_eq!(envs.len(), 1);
        assert!(envs[0].envelope.message_id.contains("r1@example.com"));
        // FTS5 picked up the body text.
        let hits = store.search("groundhog", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
