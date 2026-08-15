//! Live change events. One stream per trait — `RepoCatalog`
//! has nothing to broadcast, so its `subscribe` is omitted.
//! `IssueTracker` and `ReviewSurface` each carry their own
//! stream; on broadcast-lag the server sends a `Resync` and
//! continues, mirroring `EmailEvent::Resync`.

use crate::{IssueId, IssueState, PullRequestId, PullRequestState, RepoId};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum GitEvent {
    IssueCreated {
        repo: RepoId,
        issue: IssueId,
    },
    IssueUpdated {
        repo: RepoId,
        issue: IssueId,
        state: IssueState,
    },
    IssueCommented {
        repo: RepoId,
        issue: IssueId,
    },
    PullRequestCreated {
        repo: RepoId,
        pr: PullRequestId,
    },
    PullRequestUpdated {
        repo: RepoId,
        pr: PullRequestId,
        state: PullRequestState,
    },
    PullRequestReviewed {
        repo: RepoId,
        pr: PullRequestId,
    },
    /// Subscriber fell behind. Re-pull whatever you care about
    /// — the server has dropped older deltas.
    Resync,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::GitEvent;
    unsafe impl vox_types::Reborrow for GitEvent {
        type Ref<'a> = GitEvent;
    }
}
