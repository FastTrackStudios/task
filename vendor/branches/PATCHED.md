# branches 0.4.6, patched

The published crate plus one change, applied through `[patch.crates-io]`
in the workspace `Cargo.toml`.

**Why.** The workspace builds on a dated nightly (rust-toolchain.toml) for
the parallel front end. On a nightly rustc, upstream's `build.rs` selects
`branches_nightly`, whose no_std `abort()` calls `core::intrinsics::abort`,
which current nightlies no longer have. `turso_core` 0.8.3 (via architect's
`architect-atom`) depends on `branches` with `default-features = false` and
`^0.4.3`, and 0.4.6 is the last 0.4 release, so no published version fits.

**The change.** `build.rs` always emits `branches_stable` (plus the
`rustc_ge_*` version cfgs as before). On rustc >= 1.95 the stable path is
`core::hint::cold_path`, so `likely`/`unlikely` keep their effect. The
example, tests and bench were dropped from the copy (and from Cargo.toml).

**Drop it** when turso moves to a `branches` that builds on nightly (the
0.5 line), or the toolchain goes back to stable.
