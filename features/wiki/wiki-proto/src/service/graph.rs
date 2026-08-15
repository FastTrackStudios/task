//! 4-signal graph + Louvain + knowledge gaps.

use crate::error::WikiError;
use crate::graph::{Cluster, GraphOpts, KnowledgeGap, RelevanceScore, WikiGraph};

#[architect::rpc]
pub trait Graph {
    fn build_graph(&self, wiki_id: &str, opts: GraphOpts) -> Result<WikiGraph, WikiError>;
    fn relevance(&self, wiki_id: &str, from: &str, to: &str) -> Result<RelevanceScore, WikiError>;
    fn clusters(&self, wiki_id: &str) -> Result<Vec<Cluster>, WikiError>;
    fn gaps(&self, wiki_id: &str) -> Result<Vec<KnowledgeGap>, WikiError>;
}
