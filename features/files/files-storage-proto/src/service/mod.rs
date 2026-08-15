//! The Files placement layer's three RPC surfaces (issue #262). They are
//! three traits in three modules rather than one because they are three
//! *lanes* with three different callers — and because
//! `#[architect::rpc]` emits unqualified `serve` / `layer` helpers, so a
//! module holds exactly one service:
//!
//! - [`admin::StorageAdminService`] — the **operator lane**. Registering
//!   locations, approving agents, issuing and revoking grants. Mounted on
//!   the server lane (`/server/vox`), never on an org router: the registry
//!   is deployment-scoped and orgs never own locations.
//! - [`org::StorageService`] — the **org lane**. An org sees only what it
//!   was granted and can place only inside its grants. Mounted per org.
//! - [`agent::StorageAgentService`] — the **agent lane**: the
//!   storage-agent protocol itself. Agents announce their volumes,
//!   heartbeat health, consume a `#[subscribe]` directive stream, and
//!   report outcomes. One protocol, three hostings (glossary "Storage
//!   agent"); the in-server hosting speaks exactly this protocol
//!   in-process.
//!
//! Every method is 4 params or fewer (Facet's `#[architect::rpc]`
//! constraint, per the monorepo's root CLAUDE.md).

pub mod admin;
pub mod agent;
pub mod org;

pub use admin::StorageAdminService;
pub use agent::StorageAgentService;
pub use org::StorageService;
