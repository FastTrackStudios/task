//! Slice selector for [`crate::EmailSync::fetch_envelopes`].

use facet::Facet;

#[derive(Debug, Clone, Facet)]
#[repr(u8)]
pub enum SeqRange {
    /// Most recent `n` envelopes (newest first).
    Recent(u32),
    /// Inclusive index range, oldest-first.
    Range { from: u32, to: u32 },
    /// Every message in the folder.
    All,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::SeqRange;
    unsafe impl vox_types::Reborrow for SeqRange {
        type Ref<'a> = SeqRange;
    }
}
