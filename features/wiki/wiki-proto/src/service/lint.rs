//! Semantic + structural lint findings.

use crate::error::WikiError;
use crate::lint::{FindingAction, LintFinding, LintScope};

#[architect::rpc]
pub trait Lint {
    fn lint(&self, wiki_id: &str, scope: LintScope) -> Result<Vec<LintFinding>, WikiError>;
    fn list_findings(&self, wiki_id: &str) -> Result<Vec<LintFinding>, WikiError>;
    fn resolve_finding(
        &self,
        wiki_id: &str,
        finding_id: &str,
        action: FindingAction,
    ) -> Result<(), WikiError>;
}
