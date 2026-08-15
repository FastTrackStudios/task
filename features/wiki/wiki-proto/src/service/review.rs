//! Async review queue.

use crate::error::WikiError;
use crate::review::{ReviewAction, ReviewItem};

#[architect::rpc]
pub trait Review {
    fn enqueue_review(&self, wiki_id: &str, item: ReviewItem) -> Result<(), WikiError>;
    fn list_review(&self, wiki_id: &str) -> Result<Vec<ReviewItem>, WikiError>;
    fn apply_review(
        &self,
        wiki_id: &str,
        item_id: &str,
        action: ReviewAction,
    ) -> Result<(), WikiError>;
}
