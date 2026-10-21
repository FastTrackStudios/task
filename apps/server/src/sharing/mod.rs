//! Cross-org project sharing: the owner's half (`docs/spec/sharing.md`).
//!
//! An org that owns a project can grant ONE person from another org
//! read + comment access to that project's subtree, with no membership
//! row in the owning org. This module holds what the owner side needs:
//!
//! - [`store`] — the grant rows in `<org>/sharing.sqlite`, immutable
//!   except for revoke, and their audit lines;
//! - [`subtree`] — what a grant exposes, and the preview digest;
//! - [`SharingServiceImpl`] — the `sharing` service on the owning org's
//!   lane: preview, create, list, revoke, for owners and admins only.
//!
//! The guest's side — the grant lane that admits a person by a live
//! grant on every call — is `sharing::lane`, which is not here yet.
//! It reads grants through [`GrantStore::for_subject`] and [`is_live`].
//!
//! ## Who may call
//!
//! Only an `owner` or `admin` of the owning org, as its membership row
//! says (`t[sharing.grant.owner-only]`). Everyone else — a plain member,
//! a signed-in person with no row here, a guest holding a grant, a bot
//! (a bot is a `member` by construction, and refused by its account
//! besides), an anonymous caller — gets [`SharingError::NotFound`] from
//! every method: the same answer an owner gets for a grant id that does
//! not exist, so the service tells a non-owner nothing at all
//! (`t[sharing.grant.absent]`). Each refusal is counted in the gate's
//! deny ledger and logged once.
//!
//! The gate in front (`permits.rs`, `sharing/**`) lets members through
//! on purpose: the gate's roles cannot express owner-vs-member yet (see
//! the permits module docs on `admin`), so the decision is made here,
//! from the membership row, where it can be.

pub mod store;
pub mod subtree;

use std::sync::Arc;

use architect_permissions::Principal;
use share_proto::{
    GrantCapabilities, GrantPreview, NOT_SHARED, NewSharingGrant, SharingError, SharingGrantInfo,
    SharingService,
};
use uuid::Uuid;

pub use store::{GrantEvent, GrantEventKind, GrantStore, Revoked, SharingGrant, is_live};
pub use subtree::{ProjectTreeStub, Subtree, digest};

/// Unix seconds, now.
#[must_use]
pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One org's sharing state: its grant store, the identity that says who
/// is an owner here, and the subtree a grant exposes.
#[derive(Clone)]
pub struct Sharing {
    pub store: Arc<GrantStore>,
    /// The server's home identity — membership rows and the account
    /// store. `None` on a server with no home org, where nobody can be
    /// shown to be an owner, so every call is refused.
    pub home: Option<crate::HomeIdentity>,
    pub subtree: Arc<dyn Subtree>,
}

impl Sharing {
    /// The org slug the grants belong to.
    #[must_use]
    pub fn owner_org(&self) -> &str {
        self.store.owner_org()
    }
}

/// Emit one audit line for a grant event (`t[sharing.grant.audit]`):
/// onto the call's wide event, and as one `info` line — grant changes
/// are rare and each one is a security fact worth reading without a
/// trace query. The durable copy is the store's event row.
// t[impl sharing.grant.audit]
pub fn audit(org: &str, e: &GrantEvent) {
    use architect_telemetry::wide;
    let actor = e
        .actor
        .map_or_else(|| "system".to_owned(), |a| a.to_string());
    let org_slug = org;
    wide::set("sharing.event", e.kind.as_str());
    wide::set("sharing.grant_id", e.grant_id.to_string());
    wide::set("sharing.subject", e.subject.to_string());
    wide::set("sharing.root", e.root_id.to_string());
    wide::set("sharing.capabilities", e.capabilities.as_str());
    wide::set("sharing.preview_digest", e.preview_digest.clone());
    wide::set("sharing.actor", actor.clone());
    tracing::info!(
        target: "task_server::sharing",
        slug = %org_slug,
        event = e.kind.as_str(),
        grant_id = %e.grant_id,
        subject = %e.subject,
        root = %e.root_id,
        capabilities = e.capabilities.as_str(),
        preview_digest = %e.preview_digest,
        actor = %actor,
        "sharing: {}",
        e.kind.as_str(),
    );
}

/// Count and log a refused sharing call. The reason names the method and
/// org, never the caller's argument, so the ledger stays bounded.
fn refuse(org: &str, method: &str, who: &str) -> SharingError {
    use architect_telemetry::wide;
    let reason = format!("sharing/{method}: not an owner or admin of `{org}`");
    let org_slug = org;
    crate::permission_deny_ledger().record(&reason);
    wide::set("sharing.refused", method.to_owned());
    tracing::warn!(
        target: "task_server::sharing",
        slug = %org_slug,
        method = %method,
        principal = %who,
        "sharing: refused — not an owner or admin"
    );
    SharingError::NotFound
}

fn storage(e: &eyre::Report) -> SharingError {
    tracing::warn!(target: "task_server::sharing", error = %e, "sharing store error");
    SharingError::Storage("the sharing store could not be read".into())
}

/// A guest org's slug: what `DataRoot` accepts for an org directory.
fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// The `sharing` service on `/org/{slug}/vox`.
#[derive(Clone)]
pub struct SharingServiceImpl {
    sharing: Sharing,
}

impl SharingServiceImpl {
    #[must_use]
    pub fn new(sharing: Sharing) -> Self {
        Self { sharing }
    }

    fn org(&self) -> &str {
        self.sharing.owner_org()
    }

    /// The caller, if they are an owner or admin of this org.
    ///
    /// Reads the principal the permission gate resolved for this call
    /// (`None` off the gate is no identity, and refused), then the home
    /// membership row. A bot account is refused whatever its row says:
    /// a grant is a person's decision.
    // t[impl sharing.grant.owner-only]
    async fn admin(&self, method: &str) -> Result<Uuid, SharingError> {
        self.admin_of(architect::permissions_gate::caller(), method)
            .await
    }

    async fn admin_of(
        &self,
        caller: Option<Principal>,
        method: &str,
    ) -> Result<Uuid, SharingError> {
        use architect_auth::AuthStorage as _;
        let org = self.org();
        let Some(Principal::User { user_id }) = caller else {
            return Err(refuse(org, method, "anonymous"));
        };
        let Ok(uuid) = user_id.parse::<Uuid>() else {
            return Err(refuse(org, method, &user_id));
        };
        let Some(home) = &self.sharing.home else {
            return Err(refuse(org, method, &user_id));
        };
        let role = match home.memberships.role_for(uuid, org).await {
            Ok(Some(m)) => m.role,
            Ok(None) => return Err(refuse(org, method, &user_id)),
            Err(e) => {
                // Fail closed: an unreadable table is not a yes.
                tracing::warn!(target: "task_server::sharing", error = %e, "membership lookup failed");
                return Err(refuse(org, method, &user_id));
            }
        };
        if !matches!(role.as_deref(), Some("owner" | "admin")) {
            return Err(refuse(org, method, &user_id));
        }
        let bot = home
            .auth
            .auth
            .storage
            .find_user_by_id(uuid)
            .await
            .ok()
            .flatten()
            .and_then(|u| u.email)
            .is_some_and(|e| e.ends_with(&format!("@{}", crate::bots::BOT_DOMAIN)));
        if bot {
            return Err(refuse(org, method, &user_id));
        }
        Ok(uuid)
    }

    /// The visible set of `root` and its digest; `NotFound` when there
    /// is no such project.
    async fn preview(
        &self,
        root: Uuid,
    ) -> Result<(Vec<share_proto::VisibleItem>, String), SharingError> {
        let subtree = self.sharing.subtree.clone();
        let visible = tokio::task::spawn_blocking(move || subtree.visible(root))
            .await
            .map_err(|_| SharingError::Storage("the subtree walk failed".into()))?
            .ok_or(SharingError::NotFound)?;
        let d = digest(root, &visible);
        Ok((visible, d))
    }

    /// Record and emit `grant.expired` for any grant that has lapsed
    /// since it was last looked at.
    async fn note_expiries(&self, grants: &[SharingGrant], now: i64) {
        for g in grants {
            match self.sharing.store.note_expired(g, now).await {
                Ok(Some(e)) => audit(self.org(), &e),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(target: "task_server::sharing", error = %e, "record grant.expired");
                }
            }
        }
    }
}

impl SharingService for SharingServiceImpl {
    async fn preview_grant(
        &self,
        root_id: Uuid,
        subject: Uuid,
    ) -> Result<GrantPreview, SharingError> {
        self.admin("preview_grant").await?;
        if subject.is_nil() {
            return Err(SharingError::Invalid("a grant names a person".into()));
        }
        let (visible, digest) = self.preview(root_id).await?;
        Ok(GrantPreview {
            root_id,
            subject,
            visible,
            digest,
            not_shared: NOT_SHARED.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    async fn create_grant(&self, grant: NewSharingGrant) -> Result<SharingGrantInfo, SharingError> {
        let actor = self.admin("create_grant").await?;
        let org = self.org().to_owned();
        let now = now_unix();
        if grant.subject.is_nil() {
            return Err(SharingError::Invalid("a grant names a person".into()));
        }
        if !valid_slug(&grant.guest_org) {
            return Err(SharingError::Invalid(
                "guest_org must be the guest's org slug".into(),
            ));
        }
        if grant.guest_org == org {
            return Err(SharingError::Invalid(
                "a grant is to someone outside this org".into(),
            ));
        }
        if grant.expires_at.is_some_and(|at| at <= now) {
            return Err(SharingError::Invalid("expires_at is in the past".into()));
        }
        let GrantCapabilities::ReadComment = grant.capabilities;
        // The subject holds no membership row here: someone who already
        // belongs has their role, and a grant beside it would be a second
        // answer to what they may see.
        if let Some(home) = &self.sharing.home
            && matches!(
                home.memberships.role_for(grant.subject, &org).await,
                Ok(Some(_))
            )
        {
            return Err(SharingError::Invalid(
                "that person is already a member of this org".into(),
            ));
        }
        // t[impl sharing.grant.preview]
        let (_, current) = self.preview(grant.root_id).await?;
        if current != grant.preview_digest {
            return Err(SharingError::PreviewChanged { digest: current });
        }
        let row = SharingGrant {
            id: Uuid::new_v4(),
            owner_org: org.clone(),
            root_id: grant.root_id,
            subject: grant.subject,
            guest_org: grant.guest_org,
            capabilities: grant.capabilities,
            expires_at: grant.expires_at,
            granted_by: actor,
            granted_at: now,
            revoked_at: None,
            revoked_by: None,
            preview_digest: current,
        };
        self.sharing
            .store
            .insert(&row)
            .await
            .map_err(|e| storage(&e))?;
        audit(
            &org,
            &GrantEvent::of(&row, GrantEventKind::Created, now, Some(actor)),
        );
        Ok(row.info(now))
    }

    async fn revoke_grant(&self, grant_id: Uuid) -> Result<SharingGrantInfo, SharingError> {
        let actor = self.admin("revoke_grant").await?;
        let now = now_unix();
        let Revoked { grant, newly } = self
            .sharing
            .store
            .revoke(grant_id, actor, now)
            .await
            .map_err(|e| storage(&e))?
            .ok_or(SharingError::NotFound)?;
        if newly {
            audit(
                self.org(),
                &GrantEvent::of(&grant, GrantEventKind::Revoked, now, Some(actor)),
            );
        }
        Ok(grant.info(now))
    }

    async fn list_grants(&self) -> Result<Vec<SharingGrantInfo>, SharingError> {
        self.admin("list_grants").await?;
        let now = now_unix();
        let grants = self.sharing.store.list().await.map_err(|e| storage(&e))?;
        self.note_expiries(&grants, now).await;
        Ok(grants.iter().map(|g| g.info(now)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_org_slugs() {
        assert!(valid_slug("rockstars-of-tomorrow"));
        assert!(valid_slug("fta2"));
        for bad in ["", "-x", "x-", "Rockstars", "a b", "a/b", "a_b"] {
            assert!(!valid_slug(bad), "{bad:?}");
        }
    }
}
