//! The integration suite: two orgs, two servers, everything over iroh.
//!
//! ```sh
//! cargo nextest run -p integration
//! ```
//!
//! This is not a unit suite with a wide scope. Every crate under
//! `features/` tests itself against its own spec; what none of them can
//! test alone is the thing the product actually is — two companies, on
//! two machines, sharing one project. That is what lives here, and it
//! grows a chapter at a time as features become reachable end to end.
//!
//! # Registration is an endpoint id, not an address
//!
//! Each server binds an iroh endpoint. A client is given that endpoint's
//! id and nothing else — no host, no port, no certificate. iroh finds a
//! path, traverses the NAT, and falls back to a relay only if it must;
//! the client never learns which happened and does not need to.
//!
//! That is the whole registration model: paste an id into a device.
//!
//! # Why the fixtures are tiny
//!
//! Every byte here is written by the harness, so the fixtures are a few
//! hundred bytes each. The scenario in `docs/spec/scenario-album.md` is
//! written against 77 GB and 244 GB projects; what matters for a test is
//! the *shape* — a session folder, its media, a render, two orgs on two
//! servers — not the size. Anything that only breaks at scale needs a
//! different harness and honest labelling, not a large fixture in git.
//!
//! # Layout
//!
//! `src/` is the harness, one concept per file, so each piece can be
//! read on its own:
//!
//! | file | concept |
//! |---|---|
//! | [`server`]   | a server: one org, the real router, one endpoint |
//! | [`client`]   | a person talking to a server, over the wire |
//! | [`device`]   | a laptop: one project, partly carried |
//! | [`orgs`]     | the two companies, and what is on their disks |
//! | [`people`]   | the four accounts, and what each was granted |
//! | [`transport`]| the wire between servers |
//! | [`scenario`] | all of the above, assembled |
//!
//! Chapters call over the wire, as a signed-in person, through the
//! router a deployment mounts. The exceptions are deliberate and each
//! says so where it sits: `device` drives a laptop's own backend because
//! the laptop *is* the thing under test, `restart` reads files off disk
//! because a same-process restart can answer from a cache, and setup
//! (adoption, admission) uses the backend directly because it is
//! arranging the world rather than exercising it.
//!
//! `tests/` is the suite, one file per chapter:
//!
//! | test file | chapter |
//! |---|---|
//! | `adoption.rs`      | the tree is already there, and becomes ours |
//! | `collaboration.rs` | the two companies work on one project |
//! | `people.rs`        | four people, and what each of them may do |
//! | `device.rs`        | a laptop that syncs the audio, not the footage |
//! | `peering.rs`       | an org on as many servers as it wants |
//! | `reachable.rs`     | can a client reach any of this at all |
//! | `restart.rs`       | what survives the process |
//!
//! These are tests rather than a script that prints `ok`, because a
//! printed `ok` is only as honest as the eye reading it. A test that
//! stops asserting fails; a stage that stops asserting still prints.

pub mod client;
pub mod device;
pub mod orgs;
pub mod people;
pub mod scenario;
pub mod server;
pub mod transport;
