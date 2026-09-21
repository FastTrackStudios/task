//! The integration suite, as **one** test binary.
//!
//! Each chapter used to be its own file directly under `tests/`, which
//! cargo turns into its own binary — forty of them, every one linking the
//! whole server. Measured on 2026-09-21 with nothing else loading the
//! machine: after an edit to a mid-graph crate (`wiki-live`), compiling
//! took 7 s and relinking the forty binaries took 21 s. The chapters did
//! not need forty binaries; they needed forty *modules*.
//!
//! # Isolation is unchanged
//!
//! The suite's one structural assumption is that a process holds exactly
//! one scenario (see `integration::net`: one address book per process).
//! That was never a property of there being one binary per chapter — it
//! is a property of **nextest**, which runs every test in its own process
//! whether the tests share a binary or not. So each test still boots its
//! own world, and a subset run still proves what it proves.
//!
//! Plain `cargo test` would run a binary's tests as threads in one
//! process instead. That was already true of each chapter's several tests
//! before this change, and `net.rs` already covers it (ids do not
//! collide), so nothing new is exposed — but nextest is how the suite is
//! meant to run, and how `just ci` runs it.
//!
//! # Adding a chapter
//!
//! Write `tests/it/<chapter>.rs` and add a `mod` line below. A file under
//! `tests/` directly would quietly become a forty-first binary again.

mod adoption;
mod archive;
mod charts;
mod collaboration;
mod content_ref;
mod cross_server_wiki;
mod deliverables;
mod device;
mod form;
mod ingest;
mod keyflow_library;
mod live;
mod live_set;
mod mail;
mod merge;
mod office;
mod organise;
mod outage;
mod parts;
mod peer_to_peer;
mod peering;
mod people;
mod projects_tier;
mod reachable;
mod rebuild;
mod remote_assets;
mod restart;
mod review;
mod scale;
mod search;
mod setlist;
mod sibling_apps;
mod song_library;
mod storage;
mod studio;
mod ui_iroh;
mod vault_root;
mod versions;
mod wiki_edits;
mod wiki_promote;
mod wiki_repo;
