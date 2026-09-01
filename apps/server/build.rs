//! Rebuild when the seeded example world changes.
//!
//! `example_org.rs` embeds `examples/studio` with `include_dir!`, which
//! reads those files at compile time — but cargo has no idea it did.
//! Without the line below, editing the example tree changes nothing: the
//! crate is not stale by cargo's reckoning, so the old bytes stay
//! embedded, `admin demo` plants the previous world, and the change is
//! silently ignored until something *else* forces a rebuild.
//!
//! That is a bad failure to debug, because everything looks right. The
//! file is on disk, the seeder ran, and the thing you added is simply
//! not there. It cost two rounds of confusion the day this was written
//! — once with a test that kept passing after its fixture was deleted,
//! once with a demo server that planted a booking-shaped hole.
fn main() {
    // The whole tree: `include_dir!` walks it recursively, so any file
    // added, edited or removed anywhere under here changes the output.
    println!("cargo:rerun-if-changed=../../examples/studio");
    // And this file, so the line above can be corrected without a
    // `cargo clean`.
    println!("cargo:rerun-if-changed=build.rs");
}
