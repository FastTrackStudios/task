//! Facets, ignoring, hydration and devices — `files.facet.*`,
//! `files.ignore.*`, `files.sync.*`, `files.device.*`.
//!
//! The daemon lane. A device subscribes to a project type's **facets**,
//! not to path globs: a mix engineer takes sessions and stems, a video
//! editor takes footage and proxies. Everything unsubscribed stays
//! present as a stub and hydrates on access.
//!
//! Facets are the unit of selective sync *and* of placement policy — one
//! vocabulary for both, so "sessions on the fast NAS, proxies on cheap
//! object storage" and "this laptop takes sessions" are the same knob
//! read by two consumers.

use chrono::{DateTime, Utc};
use facet::Facet;

use crate::error::FilesFault;
use crate::id::{DeviceId, RootId};
use crate::path::RootPath;

/// A named class of content within a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet)]
#[repr(C)]
pub struct FacetName(pub String);

/// Where a facet mapping came from. The distinction matters: one is
/// knowledge about a tool, the other is a decision about a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum FacetSource {
    /// The capability knows this layout because the application always
    /// produces it — Pro Tools' `Audio Files`, Reaper's `Media`. No
    /// configuration, in any project holding the capability.
    ToolLayout,
    /// The project said so. For directories no tool created.
    Project,
    /// Nothing claims it. Syncs with the project default and is reported
    /// so the mapping can be supplied — never hidden, never guessed.
    Unmapped,
}

/// One directory's facet.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct FacetBinding {
    pub path: RootPath,
    /// `None` when [`FacetSource::Unmapped`].
    pub facet: Option<FacetName>,
    pub source: FacetSource,
    /// Subscribing to this facet implies its dependencies. A session
    /// whose audio streams in on first access will glitch, so a session
    /// arrives with the media it references.
    pub atomic: bool,
}

/// A device's subscription to a root.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Subscription {
    pub device: DeviceId,
    pub root_id: RootId,
    pub facets: Vec<FacetName>,
    /// Pinned regardless of facet — the manual override for "I am on a
    /// plane at six". Survives restart.
    pub pinned: Vec<RootPath>,
}

/// Ignore rules, in two independent layers. Neither can defeat the
/// other, and a file matching either is ignored.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct IgnoreSet {
    /// What an operating system leaves behind: `.DS_Store`, AppleDouble
    /// `._*`, thumbnail caches. Applies everywhere, whatever the
    /// capability.
    pub platform: Vec<String>,
    /// What an application leaves behind: `.rpp-bak`, peak and render
    /// caches, internal project versions.
    pub capability: Vec<String>,
    /// This root's own additions.
    pub project: Vec<String>,
}

/// Transfer limits for a device.
#[derive(Debug, Clone, Copy, PartialEq, Facet)]
#[repr(C)]
pub struct TransferPolicy {
    /// Bytes per second, `None` for unlimited.
    pub max_bytes_per_sec: Option<u64>,
    /// Suspend transfers without losing progress.
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    /// The machine's iroh endpoint id — its address and, because an
    /// iroh connection is mutually authenticated, its credential.
    ///
    /// `None` for a row from before enrolment carried one. Admission is
    /// keyed by this, so a device without it is a row the org can see
    /// and cannot sync with: revoking it would revoke nothing, which is
    /// why enrolment sets it and nothing else does.
    pub endpoint: Option<String>,
    pub last_seen: DateTime<Utc>,
    pub transfer: TransferPolicy,
    /// A revoked device is refused sync and destroys its local copy of
    /// org content on next contact.
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum SyncEvent {
    /// A path's bytes became resident, or were replaced by a stub.
    HydrationChanged(RootPath),
    SubscriptionChanged(Subscription),
    FacetsChanged(RootId),
    DeviceChanged(DeviceInfo),
    /// A device was revoked. It wipes on next contact.
    DeviceRevoked(DeviceId),
}

#[architect::rpc]
pub trait SyncService {
    /// This root's facet bindings, including the unmapped directories
    /// awaiting a decision.
    async fn facets(&self, root_id: RootId) -> Result<Vec<FacetBinding>, FilesFault>;

    /// Map a directory the tools did not create to a facet.
    async fn map_facet(
        &self,
        root_id: RootId,
        path: RootPath,
        facet: FacetName,
    ) -> Result<FacetBinding, FilesFault>;

    /// The ignore set in force, by layer.
    async fn ignore_set(&self, root_id: RootId) -> Result<IgnoreSet, FilesFault>;

    /// Replace this root's own ignore additions. The platform and
    /// capability layers are not settable here.
    async fn set_project_ignores(
        &self,
        root_id: RootId,
        patterns: Vec<String>,
    ) -> Result<IgnoreSet, FilesFault>;

    /// What this device takes from a root.
    async fn subscription(&self, root_id: RootId) -> Result<Subscription, FilesFault>;

    /// Set the facets this device takes. Content leaving the
    /// subscription becomes a stub rather than disappearing.
    async fn subscribe(
        &self,
        root_id: RootId,
        facets: Vec<FacetName>,
    ) -> Result<Subscription, FilesFault>;

    /// Pin paths for offline use regardless of facet, or unpin them.
    async fn pin(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        pinned: bool,
    ) -> Result<Subscription, FilesFault>;

    /// Fetch bytes now, or release them back to a stub.
    async fn hydrate(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        resident: bool,
    ) -> Result<Vec<RootPath>, FilesFault>;

    /// Devices registered to this org.
    async fn devices(&self) -> Result<Vec<DeviceInfo>, FilesFault>;

    /// Enrol the calling person's machine as a device of this org.
    ///
    /// The authority is the caller's own sign-in. Admission — which
    /// machines may hold this org's whole commit graph — is the widest
    /// grant the system has, so the question is who may widen it, and
    /// "a member, from a session this server issued, naming a machine
    /// they are sitting at" is an answer with a person behind it. The
    /// alternative that needs no sign-in is a machine asserting its own
    /// admission, which is not admission at all.
    ///
    /// Idempotent on `endpoint`: re-enrolling the same machine updates
    /// its name and leaves one row, because a laptop that reinstalls the
    /// app is the same laptop.
    async fn enroll_device(
        &self,
        endpoint: String,
        name: String,
    ) -> Result<DeviceInfo, FilesFault>;

    /// This org's own endpoint id — what a device sets as the peer it
    /// syncs with.
    ///
    /// Pairing needs both halves and a person should have to carry
    /// neither: [`SyncService::enroll_device`] hands the org the
    /// machine's id, and this hands the machine the org's.
    async fn coordinator(&self) -> Result<String, FilesFault>;

    /// Throttle, pause or resume a device's transfers. Pausing loses no
    /// progress.
    async fn set_transfer_policy(
        &self,
        device: DeviceId,
        policy: TransferPolicy,
    ) -> Result<DeviceInfo, FilesFault>;

    /// Cut a device off. It is refused further sync and destroys its
    /// local copy of org content on next contact — the answer to a
    /// laptop with a client's unreleased material on it.
    async fn revoke_device(&self, device: DeviceId) -> Result<DeviceInfo, FilesFault>;
}
