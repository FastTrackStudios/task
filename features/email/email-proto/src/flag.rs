//! Flag / keyword changes. The proto uses free-form strings so
//! backends can pass through IMAP keywords + JMAP custom
//! labels without translation; the well-known [`Flag`] enum
//! covers the RFC standard set.

use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum Flag {
    Seen,
    Answered,
    Flagged,
    Draft,
    Deleted,
}

/// Add/remove sets passed to [`crate::EmailSync::set_flags`].
/// Empty lists are no-ops, not errors.
#[derive(Debug, Clone, Default, Facet)]
pub struct FlagDelta {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Flag, FlagDelta};
    unsafe impl vox_types::Reborrow for Flag {
        type Ref<'a> = Flag;
    }
    unsafe impl vox_types::Reborrow for FlagDelta {
        type Ref<'a> = FlagDelta;
    }
}
