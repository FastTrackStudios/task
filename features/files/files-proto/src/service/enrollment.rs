//! Enrolling one machine with every org an account can reach, in one
//! call — the server half of an **account-driven** mount.
//!
//! Pairing is per org: [`super::SyncService::enroll_device`] hands an
//! org the machine's endpoint id, [`super::SyncService::coordinator`]
//! hands the machine the org's. A person in seven orgs did that seven
//! times, then told the sync agent about seven endpoints by hand, and
//! every org they joined afterwards was a new round of the same. What
//! they wanted was to sign in once and see everything their account
//! sees.
//!
//! This service is that. It lives on the server lane (`/server/vox`),
//! where the session token is an explicit argument rather than the
//! lane's identity, because it acts across orgs: it resolves who the
//! token is, which orgs that account is a member of (issuer-mirrored
//! and local rows alike), and enrols the endpoint in each — the same
//! per-org enrolment, run by the server on the caller's behalf, with
//! the same idempotence. The answer carries each org's own endpoint id,
//! which is everything the agent needs to admit and pull.
//!
//! It only widens what the account already has: an org the token is
//! not a member of is not in the answer and not touched.

use facet::Facet;

use crate::error::FilesFault;
use crate::id::DeviceId;

/// One org's half of a pairing, as the server made it on the caller's
/// behalf.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct OrgEnrollment {
    pub slug: String,
    pub display_name: String,
    /// The org's own endpoint id — what the machine admits and pulls
    /// from. Empty when the org has no peering endpoint yet; the
    /// enrolment still stands and the agent tries again next time.
    pub org_endpoint_id: String,
    /// The device row the org holds for this machine.
    pub device_id: DeviceId,
}

#[architect::rpc]
pub trait DeviceEnrollmentService {
    /// Enrol `endpoint` (this machine's endpoint id) with every org the
    /// session's account is a member of, naming it `name` in each org's
    /// device list, and answer each org's endpoint id.
    ///
    /// Idempotent, like the per-org enrolment it runs: a machine that
    /// asks again gets the same rows back, renamed if the name changed.
    /// An invalid or expired session is refused; an org the account is
    /// not in is not touched.
    async fn enroll_everywhere(
        &self,
        session_token: String,
        endpoint: String,
        name: String,
    ) -> Result<Vec<OrgEnrollment>, FilesFault>;

    /// What `endpoint` is already enrolled in, among the orgs the
    /// session's account is a member of. Enrols nothing.
    async fn enrollments(
        &self,
        session_token: String,
        endpoint: String,
    ) -> Result<Vec<OrgEnrollment>, FilesFault>;
}
