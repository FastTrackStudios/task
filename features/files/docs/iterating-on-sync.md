# Iterating on sync without waiting for a deploy

Sync bugs are behavioural: two machines, a network, and a clock. They do
not reproduce by reading code, and they are miserable to chase through
production, where one round trip is *merge → gate → deploy → roll → test*
and costs half an hour. Every sync bug found so far was found by putting
two real processes on one desk and watching them disagree.

This is how to get that desk in about two minutes, and what to watch.

## The desk

Two halves: a **server** with orgs in it, and a **sync agent** holding
replicas of that server's roots. Both are ordinary binaries from this
repo, both restart in seconds, and neither needs the cluster.

```bash
# The server: ACME Audio on :18080, VNT Video on :9102, seeded.
just demo serve            # owns its own data under ~/.local/share/task-demo

# The agent: its own data dir, its own port, a fast beat.
FTS_FILES_DAEMON_DATA=/tmp/dev-data \
FTS_FILES_DAEMON_ROOTS=/tmp/dev-roots \
FTS_FILES_DAEMON_BIND=127.0.0.1:4088 \
FTS_FILES_DAEMON_INTERVAL_SECS=10 \
  cargo run --release -p files-daemon --features daemon-bin --bin fts-files-daemon
```

`FTS_FILES_DAEMON_INTERVAL_SECS=10` is the whole trick. The product
default is a minute, and quiescence on top of it; ten seconds turns "wait
and see" into "watch it happen". Anything that only reproduces at the
default beat is a *timing* bug and worth saying so out loud.

Introduce them, then take the roots:

```bash
task files device pair --org acme-audio --server ws://127.0.0.1:18080/vox \
  --no-install --endpoint "$(fts-files-daemon id)" --name devbox
# prints the org's coordinator id; hand it to the agent:
FTS_FILES_DAEMON_BIND=127.0.0.1:4088 fts-files-daemon peer <coordinator-id>
```

Replicas land under `$FTS_FILES_DAEMON_ROOTS/<org>/<Name>`. **Write
there directly.** The FUSE mount is a view over exactly those
directories, so a bug that needs the mount to reproduce is a mount bug,
and every sync bug found so far reproduced without it. Skipping FUSE
removes a whole layer from the bisect.

## The loop

1. Change code.
2. `cargo build --release -p files-daemon --features daemon-bin` — a
   warm target dir makes this a couple of minutes.
3. Restart the agent. The server only needs restarting for a change
   under `apps/server` or `features/files/files`.
4. Write in a replica, watch the other side.

Watching the other side, from a third terminal:

```bash
task files browse <root-id> --org acme-audio --server ws://127.0.0.1:18080/vox
```

## What to watch, and what it means

**Read the log, do not poll the clock.** A loop that samples every
thirty seconds tells you a thing did not happen; the log tells you what
the machine decided instead, and it is usually one line.

```bash
journalctl --user -u task-sync.service -f | sed -E 's/\x1b\[[0-9;]*m//g'
```

The lines that carry the answer:

| line | meaning |
|---|---|
| `footprint … divergent_total=N` | how many paths are disputed, and the resident size. If `N` only grows, lines are forking and nothing is rejoining them. |
| `captured local work` | the cadence turned a hint into a commit. Its **absence** after you wrote something is the bug most of the time. |
| `pulled a peer's work heads=N` | a real import. `heads=0` forever means you are watching a healthy idle system. |
| `merged two lines that did not disagree` | a fork rejoined by itself. |
| `two machines changed the same files` | a real dispute, waiting for `resolve`. |

And the three states a root can be in, from `fts-files-daemon status`:
`Idle` (nothing to do), `Error` (the last pull failed — the reason is on
the line), and a percentage (a pull in flight).

## Traps that cost hours

- **Nothing was captured.** The cadence waits for quiescence, so a
  change is on disk and nowhere else for a while. If it is *never*
  captured, look at whether the root is watched at all: a replica taken
  from a peer used to arrive unwatched, and the push half then has
  nothing to send while the pull half looks perfect.
- **A pull can overwrite work that was not captured yet.** This is why
  `tick` captures before it pulls, and why that has to be a real
  invariant rather than "whatever the cadence thought was due".
- **Two lines are the normal state, not an error.** A pull leaves the
  peer's head beside your own. What matters is whether they rejoin. A
  listing that alternates between two answers is one root carrying two
  lines, not a flaky server.
- **Stop the demo server before `just ci`.** `task-cli`'s embedded-vox
  tests dial `127.0.0.1:18080` and will find your demo server instead of
  the one they started, and fail with a confusing org list.
- **Sweep the target dir.** A warm target is what makes this loop fast,
  and it reaches ~200 GB. The CI runner shares the filesystem, so a full
  disk fails *other people's* builds with `No space left on device`:
  `cargo-sweep sweep --maxsize 45` in the worktree.

## Before it goes anywhere

The behaviour you just watched belongs in a test, at the seam it
actually failed at:

- `features/files/files-sync/tests/mount_writeback.rs` — two backends
  over `LocalServer`: the right home for "an edit here shows up there".
- `features/files/files-daemon/tests/peering.rs` — two whole daemons
  over real iroh endpoints: the right home for anything about ticks,
  ordering, or peers.

Make the test fail without the fix first. A sync test that passes both
ways is testing the harness.
