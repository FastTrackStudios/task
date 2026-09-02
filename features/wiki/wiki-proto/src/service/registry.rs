//! What wikis an org holds, and the calls that change the set.
//!
//! `wiki.many.addressable` — "Listing an org returns the wikis the
//! caller may see, each with its title, purpose, visibility and page
//! count." Without this a client has to know a wiki's slug before it
//! can ask for anything, which is why every caller had `"default"`
//! hard-coded and why one org could only ever show one wiki.
//!
//! `wiki.many.set` — creating, retitling or deleting one wiki leaves
//! every other byte-identical, and an org with none is legal. The
//! mutations live here beside the listing because they are the calls
//! made *about* the set rather than about any wiki in it.
//!
//! Deliberately not per-wiki in its listing call, unlike every other
//! trait here: this is the call you make *before* you have a wiki id.

use crate::config::{NewWiki, RepoSource, Visibility, WikiConfig};
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
    /// The first paragraph of `purpose.md`, or empty. Orientation for
    /// a picker, not the document.
    #[serde(default)]
    pub purpose: String,
    /// Who may find and subscribe to it.
    #[serde(default)]
    pub visibility: Visibility,
    /// Markdown pages in it. Cheap orientation, not a guarantee.
    pub pages: u32,
    /// Whether this is the org's long-standing default tier
    /// (`wiki/Knowledge/`). One member of the set, not a privileged
    /// one — it is flagged only so a client can pick a sensible
    /// initial selection rather than the alphabetically first.
    pub default: bool,
    /// Whether a repository is the authority for its pages
    /// (`wiki.source.repo`). A client shows the commit and routes
    /// landing differently; nothing else branches on it
    /// (`wiki.source.same-surface`).
    #[serde(default)]
    pub repo_sourced: bool,
}

/// One wiki in full: its summary and its declaration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct WikiDescription {
    pub summary: WikiSummary,
    pub config: WikiConfig,
}

#[architect::rpc]
pub trait Registry {
    /// Every wiki this org holds that the caller may see.
    ///
    /// A member of the owning org sees the whole set. Anyone else sees
    /// the public ones and nothing that would reveal an unlisted or
    /// private wiki exists (`wiki.access.visibility`).
    fn list_wikis(&self) -> Result<Vec<WikiSummary>, WikiError>;

    /// One wiki, with everything it declares about itself.
    fn describe_wiki(&self, wiki_id: &str) -> Result<WikiDescription, WikiError>;

    /// Add a wiki to the set.
    ///
    /// The slug must be unused *and never used*: a slug once held by a
    /// deleted wiki is refused, because references in other people's
    /// vaults still name it (`wiki.many.identity`). The caller becomes
    /// the wiki's first Editor, which is what makes the Edit lane
    /// govern it from its first page (`wiki.edit.editor`).
    fn create_wiki(&self, new: NewWiki) -> Result<WikiSummary, WikiError>;

    /// Change who may find and subscribe to a wiki. Takes effect on
    /// what is already published: narrowing stops resolving for those
    /// who lost access, without deleting anything.
    fn set_visibility(&self, wiki_id: &str, visibility: Visibility) -> Result<(), WikiError>;

    /// Retitle. Breaks no reference and drops no subscriber, because
    /// nothing outside the wiki names it by title.
    fn set_title(&self, wiki_id: &str, title: &str) -> Result<(), WikiError>;

    /// Remove a wiki. Its slug is retired, not freed; every other wiki
    /// is untouched. Subscribers keep their local copies
    /// (`wiki.life.orphan`).
    fn delete_wiki(&self, wiki_id: &str) -> Result<(), WikiError>;

    /// Bring a repo-sourced wiki up to date with its repository now,
    /// rather than on the next scheduled fetch (`wiki.source.sync`).
    /// Returns the source as it stands afterwards — the commit
    /// reflected, or the error the fetch reported. Refused for a wiki
    /// that has no repository behind it.
    fn refresh_source(&self, wiki_id: &str) -> Result<RepoSource, WikiError>;
}
