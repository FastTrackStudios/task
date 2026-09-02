//! `OrgManagementService` server-side impl.
//!
//! Mounted at `/server/vox` (one endpoint per task-server
//! process, not per-org). Handles `create_org` calls: writes a
//! fresh `<data_root>/orgs/<slug>/` dir on disk, opens its
//! per-org SQLite DBs + runs their migrations, then hot-adds
//! the resulting [`crate::OrgAppState`] to [`AppState::orgs`]
//! so the next `/org/<slug>/...` request routes to it without
//! a server restart.
//!
//! ## Authorization
//!
//! - **Bootstrap mode** (no orgs hosted yet): unauthenticated
//!   `create_org` is allowed. The first claimed slot is
//!   typically the user's home org.
//! - **Normal mode**: `session_token` must validate against
//!   the home org's `auth.sqlite` via
//!   `ArchitectAuth::current_session`.

use std::sync::Arc;

use org_proto::{CreateOrgRequest, OrgManagementError, OrgManagementService, OrgManifest, OrgRoot};

use crate::{AppState, AuthState, build_org_state};

/// Backend that knows how to scaffold + register orgs against
/// a live [`AppState`]. Holds an `Arc<AppState>` so mutations
/// to the orgs map land on the same lock the request handlers
/// read from.
#[derive(Clone, architect::HasDispatcher)]
pub struct OrgManagementImpl {
    state: Arc<AppState>,
    /// True when served over the in-process `LocalServer` (embedded
    /// CLI): the caller already owns the data root on disk, so session
    /// validation is skipped — same trust model as the per-org
    /// embedded transport, which mounts the org router with no auth
    /// gate at all.
    local_trusted: bool,
}

impl OrgManagementImpl {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: false,
        }
    }

    /// In-process transport constructor — skips session validation
    /// (see `local_trusted`).
    #[must_use]
    pub fn new_local_trusted(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            local_trusted: true,
        }
    }
}

impl OrgManagementService for OrgManagementImpl {
    fn create_org(&self, req: CreateOrgRequest) -> Result<OrgManifest, OrgManagementError> {
        // Authorization. Bootstrap path is permissive; normal
        // mode requires a valid session token against the
        // home org's auth.sqlite.
        if !self.local_trusted && !self.state.is_bootstrap() {
            let home_slug = self.state.home_slug().ok_or_else(|| {
                OrgManagementError::Unauthorized(
                    "server has orgs but no home org — cannot validate".into(),
                )
            })?;
            if req.session_token.is_empty() {
                return Err(OrgManagementError::Unauthorized(
                    "missing session token (bootstrap mode is over — sign in to the home org)"
                        .into(),
                ));
            }
            let home = self.state.org(&home_slug).ok_or_else(|| {
                OrgManagementError::Unauthorized(format!(
                    "home org `{home_slug}` not in live dispatcher"
                ))
            })?;
            let _ = home;
            let state = self.state.clone();
            let token = req.session_token.clone();
            tokio::runtime::Handle::current()
                .block_on(async move { crate::central_auth::home_principal(&state, &token).await })
                .ok_or_else(|| OrgManagementError::Unauthorized("invalid session token".into()))?;
        }

        // Enforce single-home invariant up front so we don't
        // half-create + leave a partial org on disk.
        if req.is_home && self.state.home_slug().is_some() {
            return Err(OrgManagementError::HomeExists(
                self.state.home_slug().unwrap_or_default(),
            ));
        }

        // Scaffold the dir + write the manifest. `init_org`
        // validates the slug and refuses to overwrite an
        // existing org dir.
        let org_root: OrgRoot = self
            .state
            .data_root
            .init_org(&req.slug, &req.display_name, req.is_home)
            .map_err(|e| match e {
                org_proto::RootError::InvalidSlug { reason, .. } => {
                    OrgManagementError::InvalidSlug(reason.to_string())
                }
                org_proto::RootError::AlreadyExists { slug, .. } => {
                    OrgManagementError::AlreadyExists(slug)
                }
                other => OrgManagementError::Io(other.to_string()),
            })?;

        // Open + migrate the org's auth.sqlite. The keypair
        // mirrors the parent AppState — blob signing stays
        // consistent across orgs.
        let auth_db_url = format!("sqlite://{}?mode=rwc", org_root.auth_db().display());
        let keypair = self.state.keypair.clone();
        let scope = self.state.scope.clone();
        // The deployment's storage coordinator — a new org gets a view of
        // the same registry every other org uses, never its own.
        let storage = self.state.storage.clone();
        let slug = org_root.slug().to_owned();
        // An org created at runtime joins the same cross-org identity as
        // the ones scanned at boot — otherwise it would be the one org a
        // home principal could never reach without a restart.
        let home_identity = self.state.home_identity.clone();
        let built = tokio::runtime::Handle::current().block_on(async move {
            let auth = AuthState::open(&auth_db_url, &crate::auth_secret())
                .await
                .map_err(|e| OrgManagementError::Internal(format!("open auth: {e}")))?;
            build_org_state(
                auth,
                &keypair,
                org_root,
                &scope,
                &storage,
                home_identity.as_ref(),
            )
            .await
            .map_err(|e| OrgManagementError::Internal(format!("build org: {e}")))
        })?;

        self.state
            .insert_org(slug.clone(), built)
            .map_err(|e| OrgManagementError::Internal(e.into()))?;

        let manifest = self
            .state
            .data_root
            .org(&slug)
            .manifest()
            .map_err(|e| OrgManagementError::Io(format!("reload manifest: {e}")))?;
        Ok(manifest)
    }

    fn list_orgs(&self) -> Result<Vec<OrgManifest>, OrgManagementError> {
        let slugs = self.state.org_slugs();
        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs {
            let manifest =
                self.state.data_root.org(&slug).manifest().map_err(|e| {
                    OrgManagementError::Io(format!("load manifest for `{slug}`: {e}"))
                })?;
            out.push(manifest);
        }
        Ok(out)
    }
}
