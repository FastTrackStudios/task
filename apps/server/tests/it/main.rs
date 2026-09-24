//! `task-server`'s end-to-end tests, as **one** test binary.
//!
//! Same reasoning, same shape, as `tests/integration/tests/it/main.rs`:
//! each file directly under `tests/` is its own binary, and these were
//! fifty-three of them, every one linking the whole server. After an edit
//! anywhere below the server that is fifty-three links for one change.
//! They needed fifty-three *modules*.
//!
//! # Isolation is unchanged
//!
//! Several of these set process-wide state — `TASK_DATA_ROOT` and friends
//! through `support`, env vars the server reads at boot. That was safe
//! because each test ran in its own process, and it still does: **nextest**
//! runs every test in a process of its own whether the tests share a
//! binary or not. Plain `cargo test` would thread them together in one
//! process; nextest is how this crate's tests are meant to run, and how
//! `just ci` runs them.
//!
//! # `support` is declared once
//!
//! It was `mod support;` in each file that used it, compiled once per
//! binary. Now it is one module here, and a file that wants it writes
//! `use crate::support;`. `dead_code` stays allowed on it for the reason it
//! always was: no single test uses every helper.
//!
//! # Adding a test file
//!
//! Write `tests/it/<name>.rs` and add a `mod` line below. A file directly
//! under `tests/` would quietly become a fifty-fourth binary again.

#[allow(dead_code)]
mod support;

mod anonymous_lane_stays_anonymous;
mod asset_libraries_e2e;
mod asset_migration_e2e;
mod auth;
mod auth_live;
mod central_auth_e2e;
mod central_discovery_e2e;
mod chart_collab_e2e;
mod chart_library_e2e;
mod collection_e2e;
mod connection_identity;
mod cross_org_nodes_e2e;
mod debug_profile_e2e;
mod demo_plant;
mod device_enrollment_e2e;
mod device_sync_e2e;
mod email_migration;
mod email_to_task_e2e;
mod entity_events_stream;
mod guest_review_e2e;
mod iroh_peer_dir;
mod local_transport;
mod mcp_account_e2e;
mod mcp_chart_e2e;
mod mcp_e2e;
mod mcp_refusals_e2e;
mod mcp_telemetry_e2e;
mod mcp_wiki_e2e;
mod media_auth;
mod media_stream_e2e;
mod notify_e2e;
mod org_membership;
mod permissions_observe_only;
mod permits_cover_router;
mod personal_org_e2e;
mod plugin_toggle_e2e;
#[cfg(feature = "plugin-fasttrackstudio")]
mod live_e2e;
mod presence_relay;
mod rename_org;
mod rendition_route;
mod scheduling_durability;
mod sermon_resources_e2e;
mod share_files_e2e;
mod share_wiki_note_e2e;
mod signup_gate;
mod snapshot_async;
mod snapshot_e2e;
mod task_client_e2e;
mod vault_collab_e2e;
mod vault_graph_e2e;
mod vault_sync_e2e;
mod watch_bridge_auth;
mod webdav_auth;
mod wiki_editor_e2e;
