//! File-level types: the bytes payload, the post-PUT
//! acknowledgement, and the conditional-write mode shared by
//! [`crate::VaultSync::put_file`] and
//! [`crate::VaultSync::delete_file`].

use facet::Facet;

/// Bytes payload returned by [`crate::VaultSync::get_file`].
/// Newtype so vox has a named wire type to bind to — the orphan
/// rule blocks `unsafe impl vox_types::Reborrow for Vec<u8>`
/// from outside the vox-types crate.
#[derive(Debug, Clone, Facet)]
pub struct FileBytes(pub Vec<u8>);

/// Freshly-committed file's sha + mtime after a successful PUT.
/// The sha lets the caller record the new "last-known-server"
/// value without a follow-up GET.
#[derive(Debug, Clone, Facet)]
pub struct PutAck {
    pub sha256: String,
    pub mtime_ms: i64,
}

/// Conditional-write mode. Mirror of the original HTTP
/// `If-Match` header semantics:
/// - [`Self::CreateOnly`] — fail if the path already exists.
/// - [`Self::Sha`] — fail unless the server's current sha
///   matches.
/// - [`Self::Force`] — unconditional. Only safe for the first
///   push of a brand-new vault.
#[derive(Debug, Clone, Facet)]
#[repr(u8)]
pub enum IfMatch {
    CreateOnly,
    Sha(String),
    Force,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{FileBytes, IfMatch, PutAck};
    unsafe impl vox_types::Reborrow for FileBytes {
        type Ref<'a> = FileBytes;
    }
    unsafe impl vox_types::Reborrow for PutAck {
        type Ref<'a> = PutAck;
    }
    unsafe impl vox_types::Reborrow for IfMatch {
        type Ref<'a> = IfMatch;
    }
}
