//! The files crate's integration tests, one module per lane or chapter.
//!
//! One binary rather than one per file. Each `tests/*.rs` was its own
//! test binary, linked against the whole crate graph (jj, iroh, blobs):
//! 32 links and ~1.8 GiB of test executables in `target/` per build,
//! all of it saved into and restored from the CI cache. nextest still
//! runs every test in its own process, so nothing is shared at run time.

mod access_lane;
mod app_store;
mod cadence;
mod catalogue_offline;
mod curation_lane;
mod diag_heads;
mod federation_stream;
mod hydration_rpc;
mod lane_interop;
mod lanes_over_vox;
mod location_boundaries;
mod media_lane;
mod nested_roots;
mod op_log_compaction;
mod org_tree_rpc;
mod organise_lane;
mod restart_rpc;
mod review_lane;
mod reviews_rpc;
mod roots_lane;
mod rpc_surface;
mod search_lane;
mod software_roots;
mod structure_only_host;
mod sync_bridge;
mod sync_lane;
mod transcode_rpc;
mod tree_lane;
mod upload_lane;
mod version_lane;
mod versions_rpc;
mod write_lane;
