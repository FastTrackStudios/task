//! `IdentityService` server-side impl.
//!
//! Mounted at `/server/vox` (one endpoint per task-server
//! process, not per-org). Exposes the **home** org's identity
//! locker — the per-user set of `LinkedServer` rows holding
//! encrypted session tokens for other servers the user has
//! linked.
//!
//! ## Authorization
//!
//! Every method requires a `session_token` that validates
//! against the home org's `auth.sqlite` via
//! `ArchitectAuth::current_session`; the authenticated user's id
//! is the implicit `home_user_id` for the call. Unlike
//! [`crate::server_mgmt::OrgManagementImpl`], `local_trusted`
//! does **not** bypass the session check here: the store is
//! per-user keyed, so we always need a real `home_user_id` and
//! never forge one. `local_trusted` is retained only for
//! constructor symmetry with the other `/server/vox` services.

use std::sync::Arc;

use identity::{LinkRecord, Store};
use identity_proto::{IdentityService, IdentityServiceError, LinkServerRequest, LinkView};
use uuid::Uuid;

use crate::AppState;

/// Backend serving the home org's identity locker against a live
/// [`AppState`]. Holds an `Arc<AppState>` so it reads the same
/// orgs map the request handlers do.
#[derive(Clone, architect::HasDispatcher)]
pub struct IdentityServiceImpl {
    state: Arc<AppState>,
    /// Retained for constructor symmetry with the other
    /// `/server/vox` services. Does not relax auth — the locker is
    /// per-user, so a real `home_user_id` is always required (see
    /// module docs).
    #[allow(dead_code)]
    local_trusted: bool,
}

impl IdentityServiceImpl {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: false,
        }
    }

    /// In-process transport constructor. Note: unlike the org-mgmt
    /// service, this still validates the session token (see module
    /// docs) — a per-user locker can't run without a user id.
    #[must_use]
    pub fn new_local_trusted(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: true,
        }
    }

    /// Validate `session_token` against the home org's auth DB and
    /// return `(home identity store, home_user_id)`.
    fn resolve(&self, session_token: &str) -> Result<(Store, Uuid), IdentityServiceError> {
        let home_slug = self
            .state
            .home_slug()
            .ok_or_else(|| IdentityServiceError::Unauthorized("server has no home org".into()))?;
        if session_token.is_empty() {
            return Err(IdentityServiceError::Unauthorized(
                "missing session token".into(),
            ));
        }
        let state = self.state.clone();
        let token = session_token.to_owned();
        let home_user_id = tokio::runtime::Handle::current()
            .block_on(async move { crate::central_auth::home_principal(&state, &token).await })
            .ok_or_else(|| IdentityServiceError::Unauthorized("invalid session token".into()))?;

        let store = self
            .state
            .org(&home_slug)
            .and_then(|o| o.identity)
            .ok_or_else(|| {
                IdentityServiceError::Internal("home org has no identity locker".into())
            })?;
        Ok((store, home_user_id))
    }
}

fn record_to_view(rec: LinkRecord) -> LinkView {
    LinkView {
        id: rec.id,
        label: rec.label,
        remote_url: rec.remote_url,
        remote_slug: rec.remote_slug,
        remote_user_id: rec.remote_user_id,
        remote_email: rec.remote_email,
        token: rec.token,
        expires_at: rec.expires_at,
    }
}

/// Does this link point at an org THIS server hosts?
///
/// The locker keys links by `(remote_url, remote_slug)`, and the
/// production shape is several orgs on one host — so most links are
/// local and pushable in-process. A link to another server needs the
/// federation assertion before we can act on it, and is reported
/// `pending` rather than skipped.
fn local_slug_for(state: &AppState, link: &LinkRecord) -> Option<String> {
    state
        .org(&link.remote_slug)
        .is_some()
        .then(|| link.remote_slug.clone())
}

impl IdentityServiceImpl {
    /// The home org's own auth user — the authoritative profile.
    fn home_profile(
        &self,
        session_token: &str,
    ) -> Result<identity_proto::ProfileView, IdentityServiceError> {
        let home_slug = self
            .state
            .home_slug()
            .ok_or_else(|| IdentityServiceError::Unauthorized("server has no home org".into()))?;
        let home = self.state.org(&home_slug).ok_or_else(|| {
            IdentityServiceError::Unauthorized(format!("home org `{home_slug}` not live"))
        })?;
        let token = session_token.to_owned();
        let bundle = tokio::runtime::Handle::current()
            .block_on(async move {
                home.auth
                    .auth
                    .current_session(architect_auth::commands::CurrentSession { token })
                    .await
            })
            .map_err(|e| IdentityServiceError::Unauthorized(format!("invalid session: {e}")))?;
        Ok(identity_proto::ProfileView {
            user_id: bundle.user.id,
            email: bundle.user.email,
            name: bundle.user.name,
            image: bundle.user.image,
        })
    }
}

impl IdentityService for IdentityServiceImpl {
    fn get_profile(
        &self,
        session_token: String,
    ) -> Result<identity_proto::ProfileView, IdentityServiceError> {
        self.home_profile(&session_token)
    }

    fn sync_profile(
        &self,
        req: identity_proto::SyncProfileRequest,
    ) -> Result<identity_proto::ProfileSyncReport, IdentityServiceError> {
        let (store, home_user_id) = self.resolve(&req.session_token)?;

        // 1. Write the canonical copy first. If this fails nothing has
        //    been fanned out, so the caches stay consistent with home.
        if req.name.is_some() || req.image.is_some() {
            let home_slug = self.state.home_slug().ok_or_else(|| {
                IdentityServiceError::Unauthorized("server has no home org".into())
            })?;
            let home = self.state.org(&home_slug).ok_or_else(|| {
                IdentityServiceError::Unauthorized(format!("home org `{home_slug}` not live"))
            })?;
            let input = architect_auth::UpdateProfile {
                session_token: req.session_token.clone(),
                name: req.name.clone(),
                image: req.image.clone(),
            };
            tokio::runtime::Handle::current()
                .block_on(async move { home.auth.auth.update_profile(input).await })
                .map_err(|e| {
                    IdentityServiceError::Internal(format!("update the home profile: {e}"))
                })?;
        }

        let profile = self.home_profile(&req.session_token)?;

        // 2. Fan out to the caches. Each link carries its own session
        //    token for its org, so the push is that account updating
        //    its own profile — no impersonation, no elevated path.
        let links = tokio::runtime::Handle::current()
            .block_on(async move { store.list_links(home_user_id).await })
            .map_err(|e| IdentityServiceError::Internal(format!("list links: {e}")))?;
        let (mut updated, mut pending, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        for link in links {
            let Some(slug) = local_slug_for(&self.state, &link) else {
                pending.push(link.remote_slug.clone());
                continue;
            };
            let Some(token) = link.token.clone() else {
                failed.push(format!("{slug}: no stored token — re-run `task auth link`"));
                continue;
            };
            let Some(org) = self.state.org(&slug) else {
                pending.push(slug);
                continue;
            };
            // Mirror the whole profile, including clears: the cache
            // must converge on home, not drift toward it.
            let input = architect_auth::UpdateProfile {
                session_token: token,
                name: Some(profile.name.clone().unwrap_or_default()),
                image: Some(profile.image.clone().unwrap_or_default()),
            };
            match tokio::runtime::Handle::current()
                .block_on(async move { org.auth.auth.update_profile(input).await })
            {
                Ok(_) => updated.push(slug),
                Err(e) => failed.push(format!("{slug}: {e}")),
            }
        }

        Ok(identity_proto::ProfileSyncReport {
            profile,
            updated,
            pending,
            failed,
        })
    }

    fn list_links(&self, session_token: String) -> Result<Vec<LinkView>, IdentityServiceError> {
        let (store, home_user_id) = self.resolve(&session_token)?;
        let rows = tokio::runtime::Handle::current()
            .block_on(async move { store.list_links(home_user_id).await })
            .map_err(|e| IdentityServiceError::Internal(e.to_string()))?;
        Ok(rows.into_iter().map(record_to_view).collect())
    }

    fn link_server(&self, req: LinkServerRequest) -> Result<LinkView, IdentityServiceError> {
        let (store, home_user_id) = self.resolve(&req.session_token)?;
        let rec = LinkRecord {
            id: Uuid::nil(),
            home_user_id,
            label: req.label,
            remote_url: req.remote_url,
            remote_slug: req.remote_slug,
            remote_user_id: req.remote_user_id,
            remote_email: req.remote_email,
            token: req.token,
            expires_at: req.expires_at,
        };
        let stored = tokio::runtime::Handle::current()
            .block_on(async move { store.upsert_link(rec).await })
            .map_err(|e| IdentityServiceError::Internal(e.to_string()))?;
        Ok(record_to_view(stored))
    }

    fn unlink_server(&self, session_token: String, id: Uuid) -> Result<(), IdentityServiceError> {
        let (store, home_user_id) = self.resolve(&session_token)?;
        tokio::runtime::Handle::current()
            .block_on(async move { store.delete_link(home_user_id, id).await })
            .map_err(|e| IdentityServiceError::Internal(e.to_string()))
    }
}
