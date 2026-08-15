//! Two-step `CoT` ingest queue.

use crate::error::WikiError;
use crate::ingest::{AnalysisDraft, IngestTask, PageDraft, SourceChange};

#[architect::rpc]
pub trait Ingest {
    fn enqueue_ingest(
        &self,
        wiki_id: &str,
        source_path: &str,
        change: SourceChange,
    ) -> Result<IngestTask, WikiError>;
    fn list_ingest(&self, wiki_id: &str) -> Result<Vec<IngestTask>, WikiError>;
    fn claim_next_ingest(&self, wiki_id: &str) -> Result<Option<IngestTask>, WikiError>;
    fn record_analysis(
        &self,
        wiki_id: &str,
        task_id: &str,
        analysis: AnalysisDraft,
    ) -> Result<(), WikiError>;
    fn record_pages(
        &self,
        wiki_id: &str,
        task_id: &str,
        pages: Vec<PageDraft>,
    ) -> Result<(), WikiError>;
    fn fail_ingest(&self, wiki_id: &str, task_id: &str, error: &str) -> Result<(), WikiError>;
    fn cancel_ingest(&self, wiki_id: &str, task_id: &str) -> Result<(), WikiError>;
    fn retry_ingest(&self, wiki_id: &str, task_id: &str) -> Result<IngestTask, WikiError>;
}
