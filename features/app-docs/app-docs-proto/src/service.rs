//! The org's app docs with their faces.
//!
//! The catalog itself (register, rename, delete, list rows, the change
//! stream) is architect's `crdt::docs::Docs`, mounted beside this on the
//! same org lane. This service adds the one thing the catalog cannot
//! know: what each doc shows of itself, read from its `face`.

use uuid::Uuid;

use crate::error::FacesError;
use crate::face::DocCard;

#[architect::rpc]
pub trait AppDocFaces {
    /// The org's live docs with their faces, oldest catalog change
    /// first, at most `limit` (0 means the server's default; it caps
    /// large asks). `kind` narrows to one app's word. Tombstones are
    /// left out.
    async fn list_faces(
        &self,
        kind: Option<String>,
        limit: u32,
    ) -> Result<Vec<DocCard>, FacesError>;

    /// One doc with its face. A deleted doc is [`FacesError::Deleted`].
    async fn face(&self, doc_id: Uuid) -> Result<DocCard, FacesError>;
}
