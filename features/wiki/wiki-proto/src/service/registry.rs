//! What wikis an org holds.
//!
//! `wiki.many.addressable` — "Listing an org returns the wikis the
//! caller may see, each with its title, purpose, visibility and page
//! count." Without this a client has to know a wiki's slug before it
//! can ask for anything, which is why every caller had `"default"`
//! hard-coded and why one org could only ever show one wiki.
//!
//! Deliberately not per-wiki, unlike every other trait here: this is
//! the call you make *before* you have a wiki id.

use crate::error::WikiError;

/// One wiki, as a list a person reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct WikiSummary {
    /// The slug a reference carries, and the id every other call takes.
    pub slug: String,
    /// Display title. Falls back to the slug when the wiki has not
    /// said.
    pub title: String,
    /// Markdown pages in it. Cheap orientation, not a guarantee.
    pub pages: u32,
    /// Whether this is the org's long-standing default tier
    /// (`wiki/Knowledge/`). One member of the set, not a privileged
    /// one — it is flagged only so a client can pick a sensible
    /// initial selection rather than the alphabetically first.
    pub default: bool,
}

#[architect::rpc]
pub trait Registry {
    /// Every wiki this org holds.
    fn list_wikis(&self) -> Result<Vec<WikiSummary>, WikiError>;
}
