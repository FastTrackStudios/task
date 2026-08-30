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
fts-files-daemon shares              # every folder this machine holds
fts-files-daemon share ~/Sessions    # start holding one
fts-files-daemon unshare Sessions    # stop holding one (the files stay)
fts-files-daemon peer <id>           # sync with that machine
fts-files-daemon forget <id>         # stop syncing with it (the files stay)
fts-files-daemon checkpoint "Album"  # force a save point before unplugging
fts-files-daemon resolve Album mix.wav   # two machines changed it — keep both
```

The desktop app shows the same thing at **`/sync`**: what is syncing,
how far along, when it last did, and the paths two machines changed with
the button that settles them. It reads the agent's control socket, so
the app and the CLI cannot disagree about what is happening.

**`--roots` defaults to `~/Task`, which may not be empty.** Adopted
folders land beside whatever is already there; point it somewhere of its
own if you keep something else at that path.

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
run it on that side and re-run.

With three or more machines, name each of the others on each machine —
a root is pulled from **every** peer it was told about, so there is no
hub to designate and no order to get right. And a machine that is
asleep when you name it is fine: the intent is kept and acted on when it
answers, without running anything again.

Then share a folder from whichever machine has the files:

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

## Keeping the store on an external drive (macOS)

`--data` and `--roots` put the store and the synced projects wherever
you like, which is what you want on a machine whose internal disk is
small. On macOS there is one step that is not obvious and produces no
error message:

**A launchd agent touching an external or removable volume needs Full
Disk Access.** Without it the access does not fail — macOS waits for
consent it cannot ask a background job for, and the syscall never
returns. `launchctl` reports the service as running, the log stays
empty, and `status` says "no agent answering — is it running?"

The agent now refuses to sit there: after ten seconds it says which
directory did not answer and what to do. The fix is one grant:

1. System Settings → Privacy & Security → Full Disk Access
2. add `~/.local/bin/fts-files-daemon` (or wherever the binary lives)
3. `launchctl kickstart -k gui/$(id -u)/app.fasttrackstudio.task.sync`

Running the same binary from a terminal works without this, which is
what makes it confusing: an SSH session and a launchd agent do not get
the same answer from the same `mkdir`.

## The cloud folder: mounting a root (Linux)

Selective sync gives a machine the whole tree with only some of the
content resident — the rest sits there as a pointer stub, a few hundred
bytes recording what the file is and how big it really is. That is
honest, and until now it was also where the story stopped: a DAW that
opened a dehydrated take read the stub.

A mount closes it. The tree appears as a filesystem, everything lists at
its real size, and **opening a file this machine does not hold fetches
it first** — the caller waits as it would for a slow disk, and gets the
file. That is the Dropbox/iCloud behaviour, over Files' own stubs.

```bash
fts-files-daemon mount Ghosts ~/Task/Ghosts   # show it as a folder
fts-files-daemon mounts                        # what is mounted, and where
fts-files-daemon evict Ghosts Takes/vox.wav    # give its bytes back to the disk
fts-files-daemon fetch Ghosts Takes/vox.wav    # or bring them back by hand
fts-files-daemon unmount Ghosts                # the files stay exactly where they are
```

`evict` is the half that makes the rest worth having: a 500 GB laptop
can hold a 4 TB project as long as releasing what it is not working on
is one call and getting it back is opening the file. Nothing is lost —
the content is in the version store and on the peers.

Mounts are remembered. A machine that reboots comes back with the same
folders mounted, for the same reason it comes back syncing the same
roots: it was a decision somebody made. An agent that was killed rather
than stopped leaves the kernel holding a mount with no server behind it;
the next mount clears that itself rather than reporting a permission
error on your own directory.

Writes, renames and mkdir through the mount are ordinary writes to the
tree underneath, so the watcher, the cadence engine and checkpointing
see them exactly as they see any other edit.

## The cloud folder on macOS

macOS has no FUSE, and does not need one: the system loads a **File
Provider extension** out of the app bundle and asks *it* for material,
where on Linux the kernel asks us. Same behaviour, opposite direction.

Three pieces, in `apps/desktop/macos/FileProvider/`:

| piece | what it is |
|---|---|
| `TaskFileProvider.appex` | the extension the system loads — enumerate, fetch, write |
| `TaskFileProviderDomains` | registers one Finder folder per synced root |
| `files-fileprovider` (Rust) | the C ABI underneath: stub sizes, and the agent |

The split is deliberate. Swift owns the `NSFileProvider` callbacks
because nothing else can, and the two questions it must not answer for
itself — how big a dehydrated file really is, and how to get its bytes
— go to Rust: the first because the authority on that size is the
pointer stub, the second because **hydration writes into the store, and
the agent owns the store.** Two processes holding one jj repo is the bug
this avoids by construction. So `fetchContents` asks the agent over the
same control socket the CLI uses, and the agent materializes into the
live tree — which is on disk, so enumerating and writing are ordinary
`FileManager` work against it.

```bash
bash apps/desktop/macos/build-fileprovider.sh       # the extension alone
bash apps/desktop/macos/build-dmg.sh                # the app, with it inside
Task.app/Contents/MacOS/TaskFileProviderDomains roots   # can this Mac see the agent?
```

`roots` is the diagnostic worth knowing: registering a domain needs a
signed bundle and an entitlement, reaching the agent needs neither, and
from Finder the two failures look identical. It answers with the roots
the agent holds, or says the agent is not there.

macOS will not load an unsigned File Provider extension, so a build with
no `TEAM_ID` is for inspection only — the script says so rather than
producing something that silently does nothing.

### When you sign it

Three things bite in this order, and all three are silent:

1. **The entitlements plist must parse.** codesign hands it to AMFI,
   which reports `AMFIUnserializeXML: syntax error near line N` — no
   filename — and then signs anyway, so the entitlements are simply
   absent and the extension never loads. Both scripts now `plutil -lint`
   first. (Both files were malformed: XML forbids a double hyphen inside
   a comment, and each had a comment quoting a command-line flag.)
2. **The keychain must be unlocked.** Signing from an SSH session fails
   with `errSecInternalComponent`, which mentions neither keychains nor
   sessions. Pass `KEYCHAIN_PW`, or sign from a terminal on the machine.
3. **The app group and the extension point need a provisioning profile**
   matching the App ID. `com.apple.security.application-groups` is a
   restricted entitlement; an Apple Development identity signs the
   bundle without one, but the system will not load the result.

### Turning it on

macOS ships a third-party File Provider **disabled**, and nothing says
so where you would look: the folder appears, and every operation inside
it fails with `NSFileProviderErrorDomainDisabled` (-2011), which reaches
you as a directory listing that hangs. Dropbox and Google Drive walk
people through the switch in System Settings › General › Login Items &
Extensions › File Providers; `pluginkit -e use -i
app.fasttrackstudio.task.fileprovider` sets it directly, with no GUI,
which is what the install path does.

`TaskFileProviderDomains list` reports the state per domain, so this is
diagnosable from a terminal rather than by guessing:

```
sharetest  6baee5f2-…  on
```

### Where it actually stands

Signed with a real Developer ID certificate on airlock, and taken as far
as it goes today:

| step | state |
|---|---|
| builds, links, universal static lib | ✅ |
| Developer-ID signature, hardened runtime | ✅ `--verify --deep --strict` passes |
| the Rust half against a live agent | ✅ stub reports 200 MB where stat says 128 B |
| a domain registers | ✅ macOS creates `~/Library/CloudStorage/<app>-<root>` |
| the system loads the extension | ✅ |
| enumerating the folder in Finder | ✅ lists the project at real sizes |
| opening a dehydrated file through it | ✅ 128-byte stub → 200 MB, hash matches Linux |

`try-fileprovider.sh` is the harness that got this far — the smallest
signed app that can register a domain, so the question can be asked
without building the whole app.

Five things learned that are not obvious, each of which presents as the
same nothing:

- **The containing app must be *launched*, not merely installed.** Until
  LaunchServices has opened it once, its extensions do not count, and
  `NSFileProviderManager` answers `-2014`
  (`ApplicationExtensionNotFound`) — for at least two minutes, across
  fresh processes, while `pluginkit -m` reports the extension present
  and enabled the whole time. `open -a` fixes it instantly. For the app
  this is free; it is harnesses and scripts that get bitten.
- **A registered domain is off until it is turned on.** `pluginkit -e
  use -i <extension id>` is the switch, and does not need the GUI.
  Without it every call fails `-2011` (`DomainDisabled`) and the folder
  hangs.
- **`pluginkit -a` must be re-run after every rebuild** — its record
  points at a signature that no longer exists.
- **Nothing may block in `init(domain:)`.** Asking the agent there cost
  the launch: the system killed the process and started another, every
  fifteen seconds, with no log line from us because we never reached
  one. The lookup is lazy now, and every bridge call has a deadline — a
  filesystem extension that can hang forever is a hung Finder.
- **A sync anchor has to mean something.** Failing `enumerateChanges`
  with `syncAnchorExpired` unconditionally makes the system re-enumerate
  forever; `ls` then *times out* rather than failing, because the
  enumeration never finished, it restarted.

Ruled out along the way, so nobody re-checks them: the app-group
entitlement, a crash (no diagnostic report ever appeared), and the
binary's shape — it imports `_NSExtensionMain` from Foundation and
exports its principal class exactly as iCloud Drive's own extension
does, and both exit silently when run directly.

### The debt this leaves

The extension reads the live tree directly, which the sandbox forbids
(`NSCocoaErrorDomain 257`, "you don't have permission to view it"), so
it carries `com.apple.security.temporary-exception.files.absolute-path.read-write`
scoped to `/` — a root is wherever its owner keeps it, including an
external drive, so nothing narrower is honest.

Apple grants that for Developer-ID distribution, which is what the DMG
is, and reviews it out of App Store submissions. **So this is the reason
a Mac App Store build cannot ship**, and the way out is to stop touching
files here at all: the agent already owns the store, and an extension
that asked it to enumerate and to stage content would need no file
access whatsoever. That is the design to converge on.

Second, smaller: `fetchContents` copies the file into a staging path for
the system to take, so a 200 MB fetch moves 200 MB twice. Fine for a
first cut, worth removing when the above is done.

The app registers the domains itself, on pairing: only the containing
app may, and it is idempotent, so it runs on every pairing.

## Known gaps

- **A device's new root is adopted only if asked.** `TASK_DEVICE_SYNC_ADOPT=1`
  makes the server take on a root a device brought, landing it in the
  org's files directory. Off by default: where a tree lands is a
  placement decision, and a sweep answering it silently would let any
  admitted machine create directories in the org.
- **The File Provider extension has not been seen in Finder.** It builds
  and links on a Mac, and its Rust half is verified there against a
  running agent — `stat` reports a stub's 128 bytes where the bridge
  reports the content's 200 MB, and a fetch brings the file back. What
  is untested is the last step: a signed build, installed, showing the
  folder. That needs a Developer-ID certificate this repo's development
  machine cannot hold, so the first run on a Mac is the first real test.
  `fts-files-daemon mount` on a Mac refuses with a message saying the
  extension's job is the extension's, rather than pretending.
- **The extension re-enumerates rather than watching.**
  `enumerateChanges` reports its anchor expired, which is correct and
  merely wasteful: the system re-lists instead of being told what
  changed. Answering "nothing changed" would be the cheap lie — the
  system takes it at face value and would show a stale tree forever.
- **Windows has no service integration.** The agent runs; nothing
  registers it to start at login.
- **The DMG script has not been run.** It is written against the same
  toolchain and conventions as the working TestFlight script, and
  shellcheck is clean, but this repo's development machine is Linux —
  the first run on a Mac is the first real test of it.
