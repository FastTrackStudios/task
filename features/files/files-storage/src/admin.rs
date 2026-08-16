//! The operator lane's backend. Mounted on the server lane, never on an
//! org router: every method here is a deployment-wide act (registering a
//! location, approving an agent, admitting an org), and the registry it
//! speaks for is shared by every org in the deployment.
//!
//! # Authorization
//!
//! `/server/vox` carries no permission gate — the services on it
//! self-authenticate, each taking the session token as an argument. This
//! one does the same, through [`OperatorAuth`]: a seam rather than a
//! direct dependency, because this crate knows nothing about
//! architect-auth and should not (the server supplies an implementation
//! that validates against the home org, exactly as `OrgManagementImpl`
//! does). [`LocalTrusted`] is the in-process transport's implementation,
//! matching the `new_local_trusted` variants of the sibling services.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use files_storage_proto::{
    AgentInfo, GrantSpec, StorageAdminService, StorageError, StorageGrantInfo, StorageLocationInfo,
};
use uuid::Uuid;

use crate::core::StorageCore;
use crate::error::panicked;

/// The future an [`OperatorAuth`] check returns. Boxed because the trait
/// is used as `dyn` — validating a session is an async call into the
/// org's auth store, and an `async fn` in a trait is not dyn-safe.
pub type AuthorizeFuture<'a> = Pin<Box<dyn Future<Output = Result<(), StorageError>> + Send + 'a>>;

/// Who may act as the operator. Implemented by the server against its
/// own session store; [`LocalTrusted`] is the in-process case.
pub trait OperatorAuth: Send + Sync + 'static {
    /// `Ok(())` if this token may perform operator actions on this
    /// deployment. Anything else must be
    /// [`StorageError::Unauthorized`].
    fn authorize<'a>(&'a self, session_token: &'a str) -> AuthorizeFuture<'a>;
}

/// The in-process transport: the caller already owns the data root on
/// disk, so there is nothing a token could add — the same trust model
/// the sibling server-lane services' `local_trusted` variants use.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalTrusted;

impl OperatorAuth for LocalTrusted {
    fn authorize<'a>(&'a self, _session_token: &'a str) -> AuthorizeFuture<'a> {
        Box::pin(std::future::ready(Ok(())))
    }
}

#[derive(Clone, architect::HasDispatcher)]
pub struct StorageAdminBackend {
    core: Arc<StorageCore>,
    auth: Arc<dyn OperatorAuth>,
}

impl std::fmt::Debug for StorageAdminBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageAdminBackend")
            .finish_non_exhaustive()
    }
}

impl StorageAdminBackend {
    /// Network-facing constructor: every call is authorized by `auth`.
    #[must_use]
    pub fn new(core: Arc<StorageCore>, auth: Arc<dyn OperatorAuth>) -> Self {
        Self { core, auth }
    }

    /// In-process transport constructor — skips session validation (see
    /// [`LocalTrusted`]).
    #[must_use]
    pub fn new_local_trusted(core: Arc<StorageCore>) -> Self {
        Self {
            core,
            auth: Arc::new(LocalTrusted),
        }
    }

    /// Authorize, then run the (synchronous) work off the runtime.
    async fn run<T, F>(&self, session_token: &str, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(Arc<StorageCore>) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        self.auth.authorize(session_token).await?;
        let core = self.core.clone();
        files_store::blocking(move || f(core), panicked).await
    }
}

impl StorageAdminService for StorageAdminBackend {
    async fn list_agents(&self, session_token: String) -> Result<Vec<AgentInfo>, StorageError> {
        self.run(&session_token, |core| Ok(core.list_agents()))
            .await
    }

    async fn approve_agent(
        &self,
        session_token: String,
        agent_id: Uuid,
        approved: bool,
    ) -> Result<AgentInfo, StorageError> {
        self.run(&session_token, move |core| {
            core.approve_agent(agent_id, approved)
        })
        .await
    }

    async fn register_location(
        &self,
        session_token: String,
        agent_id: Uuid,
        volume_key: String,
    ) -> Result<StorageLocationInfo, StorageError> {
        self.run(&session_token, move |core| {
            core.register_location(agent_id, &volume_key)
        })
        .await
    }

    async fn list_locations(
        &self,
        session_token: String,
    ) -> Result<Vec<StorageLocationInfo>, StorageError> {
        self.run(&session_token, |core| Ok(core.list_locations()))
            .await
    }

    async fn issue_grant(
        &self,
        session_token: String,
        spec: GrantSpec,
    ) -> Result<StorageGrantInfo, StorageError> {
        self.run(&session_token, move |core| core.issue_grant(spec))
            .await
    }

    async fn revoke_grant(
        &self,
        session_token: String,
        grant_id: Uuid,
    ) -> Result<(), StorageError> {
        self.run(&session_token, move |core| core.revoke_grant(grant_id))
            .await
    }

    async fn list_grants(
        &self,
        session_token: String,
        org: Option<String>,
    ) -> Result<Vec<StorageGrantInfo>, StorageError> {
        self.run(&session_token, move |core| {
            Ok(core.list_grants(org.as_deref()))
        })
        .await
    }
}
