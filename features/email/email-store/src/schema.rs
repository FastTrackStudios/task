//! `SQLite` schema. Disposable: the maildir on disk is canonical;
//! `Store::rebuild_from_disk` reconstructs every row by walking
//! the tree.

pub const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    message_id   TEXT PRIMARY KEY,
    folder       TEXT NOT NULL,
    thread_id    TEXT,
    subject      TEXT NOT NULL,
    from_addr    TEXT NOT NULL,
    to_addrs     TEXT NOT NULL,
    cc_addrs     TEXT NOT NULL,
    date_ms      INTEGER NOT NULL,
    flags        TEXT NOT NULL,
    size         INTEGER NOT NULL,
    has_atts     INTEGER NOT NULL,
    snippet      TEXT,
    path         TEXT,
    content_hash TEXT
);

CREATE INDEX IF NOT EXISTS idx_messages_folder_date
    ON messages(folder, date_ms DESC);

CREATE INDEX IF NOT EXISTS idx_messages_thread
    ON messages(thread_id);

-- FTS5 over the queryable text fields. Standard content table
-- (NOT contentless) so DELETE works without the special
-- `INSERT … VALUES('delete', …)` dance. We manage the rowid
-- ourselves via a stable hash of message_id so DELETE +
-- upsert stay symmetric. Cost is ~2x storage on the text
-- columns vs a contentless table — acceptable for a cache.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    subject, from_addr, to_addrs, body_text,
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS threads (
    thread_id TEXT PRIMARY KEY,
    subject   TEXT NOT NULL,
    last_date INTEGER NOT NULL
);

"#;

/// v2 — the outbox: the staged-send state machine
/// (`Draft → PendingApproval → Approved → Sending → Sent |
/// Failed`). Replaces the never-used `pending_ops` replay queue
/// (dropped below). NOT disposable: approval state has no
/// on-disk twin, so `rebuild_from_disk` must never touch these
/// rows (it only clears `messages` / `messages_fts`).
pub const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    status          TEXT NOT NULL,
    draft_json      TEXT NOT NULL,
    origin          TEXT NOT NULL DEFAULT 'user',
    created_ms      INTEGER NOT NULL,
    updated_ms      INTEGER NOT NULL,
    retries         INTEGER NOT NULL DEFAULT 0,
    next_attempt_ms INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    sent_message_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_outbox_status
    ON outbox(status, next_attempt_ms);

-- Derivation cache: per-message computed facts (urgency, tags,
-- later summaries/draft replies). Keyed by (message_id, kind);
-- `version` stamps the computing code — stale-version rows are
-- recomputed lazily by the triage pass. The store is per
-- account, which supplies the (account, message_id, kind,
-- version) scoping the wire key describes.
CREATE TABLE IF NOT EXISTS derivations (
    message_id TEXT NOT NULL,
    kind       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    payload    TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (message_id, kind)
);

-- Notification state: first-sync baselining + alert-once. The
-- first pass over an account inserts every existing message
-- already `notified` (silent baseline — adding an account with
-- years of mail fires nothing); afterwards genuinely-new
-- messages get exactly one `notified = 0` mark (capped per
-- pass), which the notifications system drains via
-- `unnotified` / `mark_notified`.
CREATE TABLE IF NOT EXISTS notify_state (
    message_id    TEXT PRIMARY KEY,
    first_seen_ms INTEGER NOT NULL,
    notified      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_notify_unnotified
    ON notify_state(notified, first_seen_ms DESC);

CREATE TABLE IF NOT EXISTS notify_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

DROP TABLE IF EXISTS pending_ops;
"#;

/// Schema version. Bump + add a migration when the layout
/// changes; the mailbox index tables stay disposable, the
/// product tables (outbox) do not.
pub const SCHEMA_VERSION: i64 = 2;
