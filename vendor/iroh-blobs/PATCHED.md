# iroh-blobs 0.103.0, patched

This directory is the published crate `iroh-blobs` 0.103.0 (crates.io), with
one additive change. It is used through `[patch.crates-io]` in the workspace
`Cargo.toml`. The first commit that added it holds the pristine copy, so
`git diff <that commit> -- vendor/iroh-blobs` shows the whole patch.

## The change (`src/store/fs.rs`, marked `PATCH(task)`)

- New `FsStore::load_with_opts_on(db_path, options, handle)`: opens a store
  whose actors (main actor, redb meta actor, optional GC loop) run on an
  existing tokio runtime instead of a private one.
- `FsStore::load_with_opts` is unchanged in behaviour: it still builds its
  own runtime and calls the same shared code path (`load_on`).
- `Actor::new` spawns the meta actor with `tokio::spawn` (it already runs on
  the target runtime) instead of `rt.spawn`, because `rt` is empty for a
  store opened on a caller's runtime.

## Why

Upstream builds a multi-thread runtime per store, sized to the number of
cores. Task's server opens one store per root, so it ran roots × cores
threads (1,280 for 40 roots on 32 cores). Each thread may hold a glibc
malloc arena, and the free bytes parked in them grew RSS by ~250 MiB/h while
in-use heap stayed flat. `files_store::chunk::runtime` now opens all stores
on one runtime sized by `TASK_BLOB_STORE_THREADS`.

## Known, unchanged behaviour

A store opened with a GC config keeps a `Store` handle in its GC loop, so its
actor does not stop when the last user handle is dropped; call
`shutdown()` to stop it. This is the same as upstream (where it also kept the
store's private runtime and all its threads alive).

## Dropping the patch

When upstream iroh-blobs offers a way to supply the runtime, switch
`files_store::chunk::runtime::load_fs_store` to it and delete this
directory and its `[patch.crates-io]` entry.
