//! `app-docs-proto` — an app document's readable face.
//!
//! An app keeps its data as Loro docs registered in its org's app-docs
//! catalog (`crdt::docs`, served by `task_server::app_docs`). The
//! catalog row says what a doc is called and its `kind` (the app's word,
//! opaque to Task); the content is the app's own. Between the two sits
//! the **face**: a map named `face` at the doc's root, which the app
//! keeps current as it edits, and which is the only part of the content
//! Task reads.
//!
//! ```text
//! face: Map
//!   title:    String
//!   subtitle: String            (optional)
//!   colour:   String            (optional, "#22c55e")
//!   props:    List of Map { key, label, type, value }
//!             type ∈ text | number | duration | date | enum | tags | bool
//!             value: String, f64, i64, bool, or a List of String (tags)
//!   links:    List of Map { rel, label, doc }   doc = "doc:<uuid>"
//! ```
//!
//! The face describes itself: every property carries its label and its
//! type, so a reader renders any kind with no schema registered. Reading
//! is tolerant: an entry of the wrong shape or an unknown type is
//! skipped, never an error, so a newer app's face still reads in an
//! older Task.
//!
//! - [`DocFace`] and its parts: the plain decoded face.
//! - [`DocCard`]: a catalog row with its face, what a list shows.
//! - [`AppDocFaces`]: the RPC that lists an org's docs with their faces.
//! - `read_face` / `write_face` (feature `loro`): the reader the server
//!   runs, and the writer an app calls to keep its face in the shape
//!   above.

pub mod error;
pub mod face;
#[cfg(feature = "loro")]
pub mod loro_face;
pub mod service;

pub use error::FacesError;
pub use face::{DocCard, DocFace, FaceLink, FaceProp, FacePropType, FaceValue};
#[cfg(feature = "loro")]
pub use loro_face::{FACE, read_face, write_face};
pub use service::AppDocFaces;

/// The architect-generated vox client for [`AppDocFaces`], established
/// on an org's lane like every other org service.
#[cfg(feature = "vox")]
pub use service::AppDocFacesClient;
