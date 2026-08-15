//! The bulk-operation journal — what a batch changed, and how to put
//! it back.
//!
//! A bulk write is the one operation where "are you sure?" is not
//! enough. Ninety projects change in a second, the mistake is obvious
//! only afterwards, and without a record of the PRIOR values there is
//! nothing to restore from — the old values are simply gone.
//!
//! So every batch writes a journal page into the org's own vault at
//! `Records/bulk-ops/<stamp>-<op>.md`. Living in the vault (rather
//! than a dotfile on one machine) means it inherits the server's git
//! snapshots, any client can undo a batch another client applied, and
//! the record is readable markdown rather than an opaque log.
//!
//! Two properties worth stating, because both were deliberate:
//!
//! - **Written AFTER the writes, from the actual outcome.** Recording
//!   intent up front would produce a journal that disagrees with
//!   reality the moment one update fails — and undo would then try to
//!   revert changes that never happened.
//! - **Undo is itself a batch**, journalled the same way and pointing
//!   at what it reversed. An undo that isn't logged is just another
//!   unlogged mutation, which is the problem this module exists for.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::establish_for_url;

/// Where journal pages live inside an org's vault.
const DIR: &str = "Records/bulk-ops";

/// One field change on one entity — everything undo needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub id: Uuid,
    pub title: String,
    /// Value before the batch. Restoring this IS the undo.
    pub before: String,
    pub after: String,
}

/// A single applied batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRecord {
    pub batch: Uuid,
    /// `project.status`, so a future batch over another entity or
    /// field can share this journal without ambiguity.
    pub op: String,
    pub org: String,
    /// How the set was chosen, for the human reading this later.
    pub selector: String,
    pub applied_at: String,
    /// Set when this batch undoes another one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub undoes: Option<Uuid>,
    pub changes: Vec<Change>,
}

impl BatchRecord {
    fn path(&self) -> String {
        // Second precision plus the batch's short id: sortable, and
        // two batches in the same second cannot collide.
        let stamp = self.applied_at.replace([':', '-'], "").replace('.', "");
        let stamp = stamp.split('+').next().unwrap_or(&stamp);
        let short = self.batch.to_string();
        format!("{DIR}/{stamp}-{}.md", &short[..8])
    }

    /// Markdown for humans, plus a fenced JSON block undo parses.
    /// One artifact serving both readers beats a log file nobody opens
    /// and a state file nobody can read.
    fn render(&self) -> String {
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        let mut rows = String::new();
        for c in &self.changes {
            rows.push_str(&format!(
                "| {} | {} → {} |\n",
                c.title.replace('|', "\\|"),
                c.before,
                c.after
            ));
        }
        let undo_line = match self.undoes {
            Some(u) => format!("\nReverses batch `{u}`.\n"),
            None => String::new(),
        };
        format!(
            "---\ntype: bulk-op\nbatch: {}\nop: {}\norg: {}\napplied: {}\ncount: {}\n---\n\
             # Bulk operation — {}\n\n\
             `{}` · {} change(s) · selector: `{}`\n{undo_line}\n\
             | project | change |\n|---|---|\n{rows}\n\
             Undo with `task project bulk-undo --batch {}`.\n\n\
             ```json\n{json}\n```\n",
            self.batch,
            self.op,
            self.org,
            self.applied_at,
            self.changes.len(),
            self.op,
            self.applied_at,
            self.changes.len(),
            self.selector,
            self.batch,
        )
    }
}

/// Persist a batch. Returns the vault path it landed at.
///
/// A journal failure is reported to the caller rather than swallowed:
/// the writes already happened, so the operator needs to know the
/// record is missing — and gets the JSON on stdout to keep by hand.
pub async fn record(url: &str, rec: &BatchRecord) -> eyre::Result<String> {
    let client: vault_proto::VaultSyncClient = establish_for_url(url).await?;
    let path = rec.path();
    client
        .put_file(
            "default".to_owned(),
            path.clone(),
            rec.render().into_bytes(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .map_err(|e| eyre::eyre!("write journal {path}: {e:?}"))?;
    Ok(path)
}

/// Every journalled batch in the org, newest first.
pub async fn list(url: &str) -> eyre::Result<Vec<(String, BatchRecord)>> {
    let client: vault_proto::VaultSyncClient = establish_for_url(url).await?;
    let manifest = client
        .manifest("default".to_owned())
        .await
        .map_err(|e| eyre::eyre!("vault manifest: {e:?}"))?;
    let mut paths: Vec<String> = manifest
        .files
        .into_iter()
        .map(|f| f.path)
        .filter(|p| p.starts_with(DIR) && p.ends_with(".md"))
        .collect();
    // Paths lead with a sortable stamp, so lexical order is temporal.
    paths.sort();
    paths.reverse();

    let mut out = Vec::new();
    for path in paths {
        let bytes = match client.get_file("default".to_owned(), path.clone()).await {
            Ok(f) => f.0,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        // The fenced JSON block is the machine-readable half.
        if let Some(start) = text.find("```json")
            && let Some(end) = text[start + 7..].find("```")
            && let Ok(rec) = serde_json::from_str::<BatchRecord>(&text[start + 7..start + 7 + end])
        {
            out.push((path, rec));
        }
    }
    Ok(out)
}

/// Build a record from an applied batch.
#[must_use]
pub fn build(op: &str, org: &str, selector: String, changes: Vec<Change>) -> BatchRecord {
    BatchRecord {
        batch: Uuid::new_v4(),
        op: op.to_owned(),
        org: org.to_owned(),
        selector,
        applied_at: Utc::now().to_rfc3339(),
        undoes: None,
        changes,
    }
}
