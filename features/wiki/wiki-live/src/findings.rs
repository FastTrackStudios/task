//! `Wiki/_state/lint_findings.json` — open lint findings
//! produced by `agent-wiki::bridge::run_lint`. Curator
//! resolves them via the CLI or by editing the file
//! directly (it's plain JSON).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::state::StateFile;
use crate::vault::WikiLive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintKind {
    Contradiction,
    Stale,
    MissingPage,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Open,
    Resolved,
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintFinding {
    pub id: String,
    pub kind: LintKind,
    pub severity: LintSeverity,
    pub title: String,
    pub description: String,
    pub pages: Vec<String>,
    pub status: FindingStatus,
    pub raised_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct FindingsFile {
    #[serde(default)]
    pub(crate) findings: Vec<LintFinding>,
}

impl StateFile for FindingsFile {
    const FILENAME: &'static str = paths::LINT_FINDINGS_JSON;
}

impl WikiLive {
    /// Append a batch of new findings to the open list.
    /// Skips findings whose `(kind, title, pages)` triple
    /// already exists open — repeated lint passes don't
    /// duplicate.
    pub fn raise_findings(
        &self,
        items: impl IntoIterator<Item = LintFinding>,
    ) -> Result<Vec<LintFinding>, WikiLiveError> {
        let mut file: FindingsFile = self.load_state()?;
        let mut raised = Vec::new();
        for mut item in items {
            if file.findings.iter().any(|f| {
                f.status == FindingStatus::Open
                    && f.kind == item.kind
                    && f.title == item.title
                    && f.pages == item.pages
            }) {
                continue;
            }
            if item.id.is_empty() {
                item.id = format!("lint-{}", Uuid::new_v4().simple());
            }
            raised.push(item.clone());
            file.findings.push(item);
        }
        self.save_state(&file)?;
        Ok(raised)
    }

    /// List findings, optionally restricted to one status.
    pub fn list_findings(
        &self,
        status: Option<FindingStatus>,
    ) -> Result<Vec<LintFinding>, WikiLiveError> {
        let file: FindingsFile = self.load_state()?;
        Ok(file
            .findings
            .into_iter()
            .filter(|f| status.is_none_or(|s| f.status == s))
            .collect())
    }

    /// Mark a finding resolved.
    pub fn resolve_finding(&self, id: &str) -> Result<LintFinding, WikiLiveError> {
        let mut file: FindingsFile = self.load_state()?;
        let item = file
            .findings
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| WikiLiveError::TaskNotFound(format!("finding {id}")))?;
        item.status = FindingStatus::Resolved;
        item.resolved_at = Some(Utc::now());
        let snap = item.clone();
        self.save_state(&file)?;
        Ok(snap)
    }
}
