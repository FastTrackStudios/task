# Build strategy: one canonical checkout, one warm target

How agents (and the Captain) build Task on THEBATTLESHIP, and why. Every
number here was measured on THEBATTLESHIP (32 threads, 192 GB RAM, the dev
drive on btrfs `compress=zstd:1`), Rust 1.98.1, Task @ 145c8c9, October
2026. The raw logs are in the card t_025e85f1 report.

## The short version

- **One build at a time is still the right default.** A workspace test
  build alone keeps the machine at 85% CPU. Five at once took 1446 s
  each; the same five one after another took 1137 s in total. Two at once
  gained 6% throughput and nearly doubled each agent's wait.
- **Every build runs in the one canonical checkout,**
  `/run/media/Development/task`, with its in-tree `target/` (Captain,
  STA-565). Write in a light worktree if you like (`wt-new <card>`, no
  `target/`), but never build there: check your branch out in the canonical
  tree, or `gate-submit` the canonical tree with `--sha`.
- **There is no build lease** (removed in STA-567). Cargo's own
  build-directory lock serialises builds in the shared `target/`; a second
  build waits on `Blocking waiting for file lock on build directory`.
  `gate-submit` runs one gate at a time per tree and refuses a linked
  worktree.
- **Never point a second checkout at the target dir** (`CARGO_TARGET_DIR`),
  and never seed or copy a target into a worktree.
- Third-party dependencies build **without debuginfo**
  (`[profile.dev.package."*"] debug = false`); our crates keep line tables.
- Target dirs are bounded by **build kinds** (`scripts/target-sweep.py`):
  a runner keeps exactly the latest build of each kind it does; a local
  tree also keeps anything any build used in the last 7 days.

## What actually forces a rebuild

Cargo's unit hash (the `-<hash>` in every artifact name) does **not**
include the checkout's path. A worktree whose files are older than the
copied target rebuilt nothing (3.9 s no-op check). What does force a
rebuild:

1. **mtimes.** A path crate is stale when any source file is newer than
   its dep-info. `git worktree add` stamps every file "now", so a naive
   copy of the canonical target into a fresh worktree rebuilds all 188
   workspace crates (`the file … has changed (… after last build)`).
   `wt-new` gives every file whose content is identical to the canonical
   tree's the canonical file's mtime (`wt-mtime-sync`); files that differ
   keep "now". Measured: 75 and 38 crates instead of 188.
2. **The workspace version.** A release bump (0.2.0 → 0.2.2) changes every
   workspace unit's hash, so a branch on another release rebuilds all 188
   crates whatever its mtimes. That's why the canonical tree should sit at
   `origin/main`: branches start from there.
3. **Absolute paths in dep-info** for files outside a package
   (`include_dir!("../../examples/studio")`): a target built from another
   checkout points at that checkout's files, and once it's gone those
   units go stale ("… is missing"). This is one reason a shared target dir
   is wrong. The other is that two checkouts at different commits fight
   over the same unit hashes and each switch rebuilds the difference.

**Correctness rule.** A copied target is only safe in a checkout whose
files are *newer* than the target's fingerprints, or *identical* to the
files it was built from. A target dropped into an older checkout makes
cargo trust artifacts built from other sources. We saw this happen: crates
reported `Fresh` that didn't compile. This is one more reason worktrees no
longer get a target.

## Numbers

These measurements (card t_025e85f1) led to reflink-seeded worktrees,
which the Captain later dropped (STA-565): each worktree still rebuilt
what its diff touched under its own fingerprints, and the cache never
merged back. They still show why one build at a time is the default.

Cold canonical build, from an empty target:

| step | wall | CPU mean | RAM peak | target (logical / on disk) |
|---|---|---|---|---|
| `cargo check --workspace --all-targets` | 216 s | 86% | 34 GB | 13 / 10 GB |
| `cargo nextest run --profile ci --workspace --no-run` | 471 s | 85% | 40 GB | 93 / 42 GB |
| no-op check | 1.7 s | | | |

Five branches (same commit, two other release versions, two real card
branches) from a reflinked canonical target, **naive** worktrees (all 188
workspace crates rebuilt in each):

| | 5 in parallel, one worktree each | 5 serially, one tree, branch switching |
|---|---|---|
| `check --all-targets` | 168 s total (each 167–169 s), CPU 73%, RAM peak 53 GB | 189 s total (49, 48, 29, 34, 29 s), CPU 34% |
| test build (`--no-run`) | 1446 s total (each 1423–1446 s), CPU 75%, RAM peak 103 GB | 1137 s total (273, 278, 185, 205, 196 s), CPU 68% |
| disk written | +48 GB (checks), +157 GB (test builds) | +28 GB, +92 GB |

- A workspace **check** is tail-bound: alone it averages 34% CPU, with 78%
  of samples under 50%. Concurrent checks help (5 full rechecks: 168 s
  parallel vs ~240 s serial, ×1.4).
- A **test build** is not: one keeps the box busy, and five at once
  finished no sooner than five in a row (289 vs 275 s per build).

`wt-new` worktrees (reflink + mtime sync), two real card branches:

| | serial | 2 in parallel |
|---|---|---|
| branch A (75 crates): check + test build | 39 + 219 = 258 s | 46 + 419 = 465 s |
| branch B (38 crates): check + test build | 35 + 202 = 237 s | 44 + 389 = 433 s |
| total | 495 s, CPU 59% | 465 s, CPU 82%, RAM peak 49 GB |
| disk | | ~+17–20 GB per worktree after a test build |

Seeding: `cp -a --reflink=always` of a 95 GB target took 4.2–4.7 s; six
seeds together added 1.7–2.1 GB (metadata). On ext4, which has no
reflinks, the same copy would be a real 95 GB copy. On airlock (APFS)
`cp -c` clones the same way (not measured from here, because airlock isn't
reachable from THEBATTLESHIP).

Deps without debuginfo (`debug = false` for `"*"`), cold, against the
baseline above:

| | baseline | deps `debug = false` |
|---|---|---|
| cold check | 216 s | 180 s |
| cold test build | 471 s | 430 s (−9%) |
| target after check + test build | 93 GB / 42 GB on disk | 56 GB / 36 GB on disk |
| server `it` test binary | 541 MB | 375 MB (−31%) |
| edit `tests/it/main.rs` → rebuild `it` (3 runs) | 6.1–6.7 s | 6.4 s |
| backtrace | file:line | our frames still `at ./tests/…:9:56`; dependency frames by name only |

Relinking didn't get faster, because `split-debuginfo = "unpacked"`
already kept the DWARF out of the link. The win is disk and cold-build
time.

## Agent workflow

```bash
wt-new sta-123 origin/main -b fix/sta-123-thing   # write-only worktree, no target/
# … write the whole change, commit; no compiling here …
cd /run/media/Development/task
git status --porcelain                            # must be empty; else stop and ask
pgrep -af 'cargo|rustc'                           # wait if someone is building
git checkout --detach <your-sha>
nix develop --command cargo check --workspace --all-targets
git checkout -                                    # put the tree back
# or, for anything that may wait or outlast your run:
gate-submit STA-123 /run/media/Development/task ./gate.sh --sha <your-sha>
# … PR, review, merge …
wt-rm sta-123
```

## Bounding target dirs (`scripts/target-sweep.py`)

`run <kind> -- <cmd>` runs any build and records the units it used, fresh
or rebuilt (it reads fingerprint atimes, so it needs relatime, which both
hosts' dev drives have; it refuses on `noatime`). `sweep` deletes every
unit no kind's latest build used.

- **Persistent CI target dirs** (the airlock Mac): every build step runs
  as a kind (`macos`, `ios`, `desktop-release`), and the job ends with
  `sweep --protect-hours 0`. So the runner keeps its latest build of every
  kind it does and nothing else. This replaced `cargo sweep --maxsize`,
  which evicts oldest-first. Once a run's working set outgrew the cap, it
  evicted that run's own dependencies, and the next run rebuilt them from
  cold. (checks.yml's lint, tests and wasm run on GitHub-hosted runners
  with a throwaway `./target` and a rust-cache saved from main, so they
  need no sweep.)
- **Local trees** (the canonical checkout): `just sweep-local`, which is
  `sweep --keep-days 7 --flycheck-max-gb 40`. The Captain's `dx serve`, the
  IDE and bare `cargo run` aren't wrapped as kinds, so anything any build
  read in the last 7 days stays. Measured on a reflinked canonical copy
  after a check and a test build: the CI policy freed 39 GB (32 GB of it
  stale incremental), and the next check and test build were no-ops
  (1.5 s, 3.0 s).
- **rust-analyzer** keeps its own `target/flycheck0`. Leave it separate.
  Pointed at `target/` itself, the IDE would queue behind every agent's
  build on cargo's lock, and its diagnostics would stall for the length of
  a test build. It's a check-only tree (~13 GB), capped whole at 40 GB.
- Run `just sweep-local` from a daily timer, plus whenever the dev drive
  has less than 200 GB free.
