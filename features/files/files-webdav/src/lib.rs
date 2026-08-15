//! The Files **WebDAV bridge** (issue #274, part of the Files platform
//! spec #255): `dav-server` over a custom filesystem view of an org's
//! File Roots, so any OS file manager can mount a root read-write
//! without the sync daemon installed — user story 30, "on a machine
//! without the daemon, mount my roots' live trees".
//!
//! What this is, precisely (spec, *Surfaces*): "a compat bridge, never
//! the sync path". Four properties define it, and each has a home in
//! this crate:
//!
//! - **Current heads only.** `LiveTreeFs` serves a
//!   root's live tree with the root's own internals — marker file and
//!   version store — removed from existence. There is no
//!   version-addressed URL space at all; version history stays behind
//!   `FilesService::chain`.
//! - **Read-write, and writes are ordinary writes.** A `PUT` lands in
//!   the live tree exactly like a DAW saving over NFS would, so the
//!   scan-certifying cadence pipeline picks it up on the next Session
//!   checkpoint. The bridge has no privileged write path of its own.
//! - **Mountable.** [`WebdavBridge`] presents the org's roots as
//!   one collection (`RootsFs`) and gives each root a real
//!   lock manager, which is what macOS and Windows require before they
//!   will mount a share read-write.
//! - **Governed.** [`WebdavPolicy`] hides a root from WebDAV;
//!   hidden roots are indistinguishable from absent ones.
//!
//! **Auth is the host router's job.** WebDAV clients speak plain HTTP,
//! not vox, so identity cannot ride a WS upgrade here — the mounting
//! route authenticates (session bearer, HTTP Basic, or a signed token)
//! and org-scopes the request *before* calling
//! [`WebdavBridge::handle`], the same shape `/media` uses. This
//! crate never sees a credential, and deliberately holds no policy that
//! would let it be mounted unauthenticated by accident.

mod bridge;
mod live_tree_fs;
mod naming;
mod policy;
mod roots_fs;

pub use bridge::WebdavBridge;
pub use policy::WebdavPolicy;

/// Re-exported so a host router can name the body type
/// [`WebdavBridge::handle`] returns without depending on `dav-server`
/// directly.
pub use dav_server::body::Body;
