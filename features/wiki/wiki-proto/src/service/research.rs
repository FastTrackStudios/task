//! Deep Research plans.

use crate::error::WikiError;
use crate::ingest::IngestTask;
use crate::research::{RawSource, ResearchPlan, ResearchSourceKind, ResearchStatus};

#[architect::rpc]
pub trait Research {
    fn propose_research(
        &self,
        wiki_id: &str,
        source_kind: ResearchSourceKind,
        source_id: &str,
    ) -> Result<ResearchPlan, WikiError>;
    fn list_research(&self, wiki_id: &str) -> Result<Vec<ResearchPlan>, WikiError>;
    fn set_research_status(
        &self,
        wiki_id: &str,
        plan_id: &str,
        status: ResearchStatus,
    ) -> Result<(), WikiError>;
    fn submit_research_result(
        &self,
        wiki_id: &str,
        plan_id: &str,
        sources: Vec<RawSource>,
    ) -> Result<Vec<IngestTask>, WikiError>;
}
