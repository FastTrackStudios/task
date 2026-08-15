//! `Wiki/log.md` — append-only operation timeline.

use std::fs;
use std::io::Write;

use chrono::{DateTime, Utc};
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

/// What kind of operation an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogOp {
    Ingest,
    Query,
    Lint,
    Review,
    Research,
    Admin,
}

impl LogOp {
    fn slug(self) -> &'static str {
        match self {
            LogOp::Ingest => "ingest",
            LogOp::Query => "query",
            LogOp::Lint => "lint",
            LogOp::Review => "review",
            LogOp::Research => "research",
            LogOp::Admin => "admin",
        }
    }
}

/// One log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    pub op: LogOp,
    pub title: String,
    pub body: String,
    pub pages_touched: Vec<String>,
}

impl WikiLive {
    /// Append an entry to `Wiki/log.md`. Creates the file
    /// if missing.
    pub fn append_log(&self, entry: LogEntry) -> Result<(), WikiLiveError> {
        let path = self.wiki_root().join(paths::LOG_MD);
        if !path.is_file() {
            ensure_log(self)?;
        }
        let date = entry.at.format("%Y-%m-%d");
        let mut body = String::new();
        body.push_str(&format!(
            "\n## [{date}] {op} | {title}\n",
            op = entry.op.slug(),
            title = entry.title
        ));
        if !entry.body.is_empty() {
            body.push('\n');
            body.push_str(entry.body.trim_end());
            body.push('\n');
        }
        if !entry.pages_touched.is_empty() {
            body.push_str("\n**Pages touched:** ");
            body.push_str(
                &entry
                    .pages_touched
                    .iter()
                    .map(|p| format!("[[{p}]]"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            body.push('\n');
        }

        let mut f = fs::OpenOptions::new().append(true).open(&path)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        Ok(())
    }
}

pub(crate) fn ensure_log(wiki: &WikiLive) -> Result<bool, WikiLiveError> {
    let path = wiki.wiki_root().join(paths::LOG_MD);
    if path.is_file() {
        return Ok(false);
    }
    fs::write(
        &path,
        "# Wiki log\n\nAppend-only timeline. Entries: `## [YYYY-MM-DD] <op> | <title>`.\n",
    )?;
    Ok(true)
}
