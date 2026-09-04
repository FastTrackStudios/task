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

use crate::config::{NewWiki, PendingPush, RepoSource, Visibility, WikiConfig};
use crate::error::WikiError;

/// How a page of a repo-sourced wiki differs from its base commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum ChangeKind {
    /// Not in the repository at the base; here now.
    Added,
    /// In both; the content differs.
    Modified,
    /// In the repository at the base; gone here.
    Deleted,
}

impl ChangeKind {
    /// The word a person reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// One page the working copy has changed since its base.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct LocalChange {
    /// Wiki-relative path, `guide/setup.md`.
    pub path: String,
    pub kind: ChangeKind,
}

/// What a repo-sourced wiki holds that its repository does not yet
/// (`wiki.source.editable`). Derived from the pages on disk against the
/// tree exported at `base_commit`, never tracked by hand — so an edit
/// made through any door (the app, the CLI, a mounted folder) counts.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct LocalChanges {
    /// The commit the working copy was exported from.
    pub base_commit: String,
    /// Sorted by path.
    pub changes: Vec<LocalChange>,
    /// The push awaiting merge, if any — the branch these changes will
    /// be pushed onto next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingPush>,
    /// Pages a sync could not update because both sides changed them.
    /// A push is refused while any stand.
    #[serde(default)]
    pub conflicts: Vec<String>,
}

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
    /// Where the wiki's tree lives, relative to the org root —
    /// `wikis/<slug>` for a created wiki, `wiki/Knowledge` for the
    /// default tier. A tool that runs beside the data (the CLI's
    /// FS-only verbs against an embedded or local org) joins this to
    /// the org root instead of guessing the layout; a remote client
    /// has nothing to join it to and ignores it. Absolute only when
    /// the backend has no org root to be relative to (a single-root
    /// test); empty on a server older than this field.
    #[serde(default)]
    pub root: String,
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

    /// What a repo-sourced wiki's working copy holds that the
    /// repository does not: every page added, changed or removed since
    /// the base commit, the push awaiting merge if any, and the pages
    /// a sync left in conflict (`wiki.source.editable`). Refused for a
    /// wiki with no repository behind it.
    fn local_changes(&self, wiki_id: &str) -> Result<LocalChanges, WikiError>;

    /// Send the working copy's changes to the repository as one commit
    /// on one branch, and open a pull request for it when the forge
    /// allows; a push while one is pending rewrites that branch and its
    /// request rather than opening another. Editor only, and made as
    /// the caller's own forge identity — an Editor the forge does not
    /// know is refused before anything is pushed. Refused with nothing
    /// pushed when there are no changes or any conflict stands.
    /// `title` is the commit's subject and the request's title; `body`
    /// the rest of both.
    fn push_changes(
        &self,
        wiki_id: &str,
        title: &str,
        body: &str,
    ) -> Result<PendingPush, WikiError>;
}
