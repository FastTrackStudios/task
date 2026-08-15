//! `RepoCatalog` — repo-level metadata. Bindings depend on
//! these; nothing else.

use crate::{GitError, Repo, RepoId};

#[architect::rpc]
pub trait RepoCatalog {
    /// Repos this backend can address. May be paginated by the
    /// caller via repeated `list_repos` calls — first pass
    /// returns everything the credential can see.
    fn list_repos(&self) -> Result<Vec<Repo>, GitError>;

    /// One repo by id. `GitError::NotFound` if the credential
    /// can't see it (forges hide this behind 404 deliberately).
    fn get_repo(&self, repo: &RepoId) -> Result<Repo, GitError>;
}

#[cfg(feature = "vox")]
pub use self::RepoCatalogClient as Client;
