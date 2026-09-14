//! `DeviceEnrollmentService` on `/server/vox` — enrol one machine with
//! every org an account can reach.
//!
//! The per-org half already exists: each org's `files` backend answers
//! [`files_proto::SyncService::enroll_device`] and
//! [`files_proto::SyncService::coordinator`]. What was missing was the
//! step a person did by hand, once per org: find out which orgs the
//! account is in and run the pairing in each. That is all this does,
//! and it does it with the same calls, so an enrolment made here and
//! one made with `task files device pair` are the same row.
//!
//! Membership is the home org's table — the rows the issuer mirrors
//! plus the ones granted locally — resolved the way the account lane
//! resolves it ([`crate::central_auth::home_principal`]). A token that
//! resolves to nobody is refused; an org the account is not in is not
//! in the answer.

use std::sync::Arc;

use files_proto::error::FilesFault;
use files_proto::service::enrollment::{DeviceEnrollmentService, OrgEnrollment};
use files_proto::service::sync::SyncService as _;

use crate::AppState;

#[derive(Clone, architect::HasDispatcher)]
pub struct DeviceEnrollmentImpl {
    state: Arc<AppState>,
}

impl std::fmt::Debug for DeviceEnrollmentImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceEnrollmentImpl")
            .finish_non_exhaustive()
    }
}

impl DeviceEnrollmentImpl {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    /// The org slugs the session's account is a member of, or a refusal
    /// when the token resolves to nobody.
    async fn member_slugs(&self, session_token: &str) -> Result<Vec<String>, FilesFault> {
        if session_token.trim().is_empty() {
            return Err(denied("a session token is required"));
        }
        let Some(home) = &self.state.home_identity else {
            return Err(denied(
                "this server has no home org to resolve accounts against",
            ));
        };
        let Some(user_id) = crate::central_auth::home_principal(&self.state, session_token).await
        else {
            return Err(denied("the session token names nobody this server knows"));
        };
        let rows = home
            .memberships
            .for_user(user_id)
            .await
            .map_err(|e| FilesFault::Io(format!("memberships: {e}")))?;
        let mut slugs: Vec<String> = rows.into_iter().map(|m| m.org_slug).collect();
        slugs.sort();
        slugs.dedup();
        Ok(slugs)
    }

    /// One org's answer, given the device row the org holds (or just
    /// made) for the endpoint.
    async fn enrollment_of(
        &self,
        slug: &str,
        device: files_proto::service::sync::DeviceInfo,
    ) -> Option<OrgEnrollment> {
        let org = self.state.org(slug)?;
        let display_name = self
            .state
            .data_root
            .org(slug)
            .manifest()
            .map(|m| m.display_name)
            .unwrap_or_else(|_| slug.to_owned());
        // No endpoint yet is not a failure of the enrolment: the row
        // stands, and the agent asks again on its next round.
        let org_endpoint_id = org.files.coordinator().await.unwrap_or_default();
        Some(OrgEnrollment {
            slug: slug.to_owned(),
            display_name,
            org_endpoint_id,
            device_id: device.id,
        })
    }
}

fn denied(why: &str) -> FilesFault {
    FilesFault::Denied {
        action: format!("enroll: {why}"),
        path: files_proto::path::RootPath::root(),
    }
}

impl DeviceEnrollmentService for DeviceEnrollmentImpl {
    async fn enroll_everywhere(
        &self,
        session_token: String,
        endpoint: String,
        name: String,
    ) -> Result<Vec<OrgEnrollment>, FilesFault> {
        if endpoint.trim().is_empty() {
            return Err(FilesFault::Io(
                "a device must present an endpoint id".into(),
            ));
        }
        let name = if name.trim().is_empty() {
            "a machine".to_owned()
        } else {
            name
        };
        let mut out = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        for slug in self.member_slugs(&session_token).await? {
            let Some(org) = self.state.org(&slug) else {
                continue;
            };
            let device = match org
                .files
                .enroll_device(endpoint.clone(), name.clone())
                .await
            {
                Ok(device) => device,
                Err(e) => {
                    refused.push(format!("{slug}: {e}"));
                    continue;
                }
            };
            if let Some(enrollment) = self.enrollment_of(&slug, device).await {
                out.push(enrollment);
            }
        }
        architect_telemetry::wide::set("files.enroll.orgs", out.len() as i64);
        if !refused.is_empty() {
            architect_telemetry::wide::set("files.enroll.refused", refused.join(", "));
        }
        Ok(out)
    }

    async fn enrollments(
        &self,
        session_token: String,
        endpoint: String,
    ) -> Result<Vec<OrgEnrollment>, FilesFault> {
        let mut out = Vec::new();
        for slug in self.member_slugs(&session_token).await? {
            let Some(org) = self.state.org(&slug) else {
                continue;
            };
            let Ok(devices) = org.files.devices().await else {
                continue;
            };
            let Some(device) = devices
                .into_iter()
                .find(|d| !d.revoked && d.endpoint.as_deref() == Some(endpoint.as_str()))
            else {
                continue;
            };
            if let Some(enrollment) = self.enrollment_of(&slug, device).await {
                out.push(enrollment);
            }
        }
        Ok(out)
    }
}
