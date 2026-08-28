# Syncing a project between machines

What this describes: a server, a desktop, and a laptop holding the same
projects, converging in both directions, with no interactive login and no
app window open.

## The shape

Three things, and only the middle one is new:

- **The engine** (`features/files/files-sync`) — commit graphs plus
  chunk transfer, resumable, content-verified on arrival. Divergence is
  preserved as sibling heads and resolved by a person; sync never merges
  content.
- **The agent** (`features/files/files-daemon`, binary
  `fts-files-daemon`) — the engine as a long-lived background service
  with a persistent device identity. It *serves* its own replica lane
  and *pulls* from its coordinator.
- **The server sweep** (`apps/server/src/device_sync.rs`) — the org
  pulling its admitted devices on a timer.

There is no push anywhere in this. Sync in both directions is two pulls,
which is why every machine has to be dialable: a laptop nothing can dial
has no way to hand over the work it did offline, and the failure is
silent, because a call nobody makes raises no error.

## Identity and admission

A machine's identity is its **iroh endpoint key**, on disk in its data
dir. The public half — its endpoint id — is its address and its
credential at once: an iroh connection is mutually authenticated, so
dialling proves who you are with nothing to mint, store, expire or leak.

Admission is therefore a list, not a secret, and it is symmetric: each
side admits the other's endpoint id.

**Normally you do none of that.** Open the desktop app and sign in: it
asks the local agent for its endpoint id, calls `enroll_device` on the
org — which admits the machine on the authority of your sign-in — asks
the org for its own id in return, and points the agent at it. Once per
launch, no prompt, and the log line says what happened. The org's device
list then shows the machine by hostname, and revoking it there cuts it
off for real (the endpoint stops being admitted, which is what a pull is
actually gated on).

For a machine with no app to sign into — a studio rig, a build box, a
server — the CLI does the same exchange on the strength of its own
stored session:

```
task files device pair              # enrol this machine, install its agent
task files device pair --endpoint <id> --no-install   # pair another machine
task files device list
task files device revoke <device-id>
```

Revoking there is real: the endpoint stops being admitted, which is what
a pull is gated on, so the machine is refused on its next contact rather
than merely flagged.

The fully manual path still exists for bringing up a first server, or
recovering an org whose app access is the broken thing:

| where | what to do |
|---|---|
| on the device | `fts-files-daemon id` prints its endpoint id |
| on the server | add that line to `<data-root>/orgs/<slug>/admitted-devices` |
| on the device | point it at the org: `--coordinator <org endpoint id>` |

The org's own id is in `<data-root>/orgs/<slug>/iroh-endpoint-id`, written
at first boot and stable across restarts.

The server re-reads `admitted-devices` on every sweep, so admitting and
revoking take effect without a restart. Deleting a line dismisses that
device — including across a restart, which is what
`admitted-devices.applied` beside it is for. Peers admitted by anything
else (federation admits server peers into the same table) are left alone.

## Installing the agent

```
fts-files-daemon install --coordinator <org endpoint id>
fts-files-daemon install --dry-run          # print what it would do
fts-files-daemon service-status
fts-files-daemon uninstall
```

And once it is running:

```
fts-files-daemon id                  # this machine's endpoint id
fts-files-daemon status              # what it is syncing, and how far along
fts-files-daemon checkpoint "Album"  # force a save point before unplugging
```

It registers a **user** agent — a launchd `LaunchAgent` on macOS, a
systemd user unit on Linux — that starts at login and restarts if it
dies. A user agent rather than a system daemon because it syncs one
person's files into one person's home under one person's device
identity; the cost is that it starts at login, which `loginctl
enable-linger $USER` closes on Linux.

The desktop app installs it for you on first run
(`apps/desktop/src/sync_service.rs`) from the copy shipped beside it —
inside `Task.app/Contents/MacOS/` on macOS. `TASK_SYNC_AUTOSTART=0`
turns that off; an agent already installed is never touched.

### Configuration

Everything is environment-driven, so the unit, a shell and a container
all configure it the same way:

| var | meaning | default |
|---|---|---|
| `FTS_FILES_DAEMON_DATA` | store, vault, identity, endpoint key | `~/.local/share/fts-files` |
| `FTS_FILES_DAEMON_ROOTS` | where synced projects land | `~/Task` |
| `FTS_FILES_DAEMON_COORDINATOR` | the org endpoint id to sync with | — |
| `FTS_FILES_DAEMON_BIND` | control socket for the app | `127.0.0.1:4055` |
| `FTS_FILES_DAEMON_INTERVAL_SECS` | reconcile cadence | `30` |
| `FTS_FILES_DAEMON_SYNC_ALL` | take everything the org offers | `1` |
| `FTS_FILES_DAEMON_SNAPSHOT_SECS` | cadence: debounce before a snapshot | `600` |
| `FTS_FILES_DAEMON_QUIESCE_SECS` | cadence: quiet before a checkpoint | `1800` |

The last two are worth understanding, because they set how *long* work
sits on a machine before another one can have it: nothing syncs that has
not been captured, and cadence decides when local edits become a save
point. The defaults are tuned for legible version history — one save
point per session rather than one per keystroke — which on a machine
whose job is handing work to another machine is a long time to hold it.
A laptop that should feel closer to continuous wants both much lower;
`checkpoint` forces one immediately.

On the server, `TASK_DEVICE_SYNC_SECS` sets the sweep interval (`0`
disables the sweep and leaves admission alone).

## Run the server with `TASK_ENFORCE_PERMISSIONS=1`

Admission and revocation only *bite* when the org's permission gate is
enforcing. The default is observe-only: the gate evaluates every call,
records what it would have refused, and allows it through — which is the
right default for bringing up permit tables, and the wrong one for a
server holding real content. Without it:

- an endpoint the org never admitted can still pull the commit graph, and
- revoking a device records the revocation without refusing the device.

With it, both work, and this was checked rather than assumed: against an
enforcing server, an unauthenticated `task files device pair` is refused
(`permission denied … files-sync/enroll_device`), a signed-in one
succeeds, the paired machine replicates the projects, and after
`task files device revoke` the same machine is refused on its next tick
(`permission denied … files-replica/roots`).

## What a tick does

1. **Capture.** The agent watches its roots and runs the cadence pass, so
   a session that has gone quiet becomes a checkpoint. Without this the
   pull half would reconcile against a history that never moves.
2. **Pull.** Every chosen root reconciles against its peer: missing
   commits, then only the chunks this machine lacks — which is what makes
   an interrupted transfer resume rather than restart.

Selective sync is by facet, not path: a device subscribes to the kinds of
work it does, and what it did not ask for stays present as a stub with its
real name and size, hydrating on access.

## Try it

```
just demo serve     # two orgs, two servers, endpoint ids minted
just demo daemon    # a third machine: the agent, replicating ACME
```

`demo daemon` performs both admissions for you and prints every id it
used. Edit a file under the device's `roots/` directory, wait a sweep,
and it appears in the server's tree.

## Putting it on a second machine

Until a Developer-ID build exists, the fastest route is to build the
agent where it will run:

```
cargo build --release -p files-daemon --features daemon-bin --bin fts-files-daemon
./target/release/fts-files-daemon install       # launchd agent / systemd user unit
./target/release/fts-files-daemon id            # this machine's endpoint id
```

Then introduce the two machines — on each, naming the other:

```
fts-files-daemon peer <the other machine's endpoint id>
```

The first one to run it is told the other side has not admitted it yet;
run it on that side and re-run. Then share a folder from whichever
machine has the files:

```
fts-files-daemon share ~/Music/Sessions
```

**A snag you will hit before any of that**: this repo's `[patch.crates-io]`
points `vox-core` at an absolute path on one machine
(`/run/media/Development/vox-global-mw`), so a checkout anywhere else
fails to build with `failed to read …/vox-core/Cargo.toml`. The branch
it patches in exists only in that local worktree; pushing
`feat/global-client-middleware` to the vox repo and referencing it by
git is what makes this repo buildable on a second machine. Copying the
worktree across and repointing the patch locally works meanwhile.

## Shipping it on macOS

`just dmg` (on a Mac) builds a **Developer-ID disk image**: hardened
runtime, notarized, stapled, with the agent inside the bundle. Drag Task
to Applications, open it, sign in — the agent installs itself and the
machine pairs.

Not the App Store build, and the reason is structural rather than
preference: a sandboxed app may not write into `~/Library/LaunchAgents`,
so it cannot register the agent that keeps syncing when the window
closes. `apps/desktop/macos/deploy-testflight-macos.sh` still builds the
App Store version; it just cannot install background sync, and shipping
sync through it would need `SMAppService` with the plist inside the
bundle.

Signing needs a "Developer ID Application" certificate in the keychain —
a different kind from the App Store identities `setup-macos.sh`
provisions, and a one-time download from developer.apple.com.
`DRY_RUN=1 just dmg` stops at a signed, un-notarized image for
iterating.

## Known gaps

- **A device's new root is adopted only if asked.** `TASK_DEVICE_SYNC_ADOPT=1`
  makes the server take on a root a device brought, landing it in the
  org's files directory. Off by default: where a tree lands is a
  placement decision, and a sweep answering it silently would let any
  admitted machine create directories in the org.
- **Windows has no service integration.** The agent runs; nothing
  registers it to start at login.
- **The DMG script has not been run.** It is written against the same
  toolchain and conventions as the working TestFlight script, and
  shellcheck is clean, but this repo's development machine is Linux —
  the first run on a Mac is the first real test of it.
