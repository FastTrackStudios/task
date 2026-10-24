# A headless Task node

Task has no central server. Every native install is a node — the desktop
app and the iPhone app each run the backend themselves and sync with each
other directly over iroh (docs/spec/offline.md, `offline.peers`). A
**headless node** is the same thing with no window: the `task-server`
binary, left running on a desktop, a home server, a VPS or the cluster, so
that one of your nodes is always on and always holds your data. The
production pod is one such node; yours is just another.

It is one binary, the same for "run it on my desktop" and "run it on a
server", and it is versioned with everything else: `task-server
--version` prints the release it came from.

```text
task-server --version            task-server 0.2.3   (prints and exits; never opens data or binds a port)
task-server peer sign-in         sign this node in as your account
task-server peer                 run as one more of your devices
task-server                      host the orgs under $TASK_DATA_ROOT (the cluster's mode)
task-server service write <dir>  the systemd unit / launchd agent that keeps it running
task-server self-update          move to the newest release, signature- and sha256-checked
```

## Two modes

- **`task-server peer` — your device, always on (the default here).** It
  is signed in as your FastTrackStudio account, joins the account's shared
  org from a device that holds it, admits your other devices and is
  admitted by them (`offline.server-peer`, `offline.account`). Nothing is
  configured by hand: no endpoint ids, no org slugs. Use this on a home
  server or a desktop that stays on.
- **`task-server` — host orgs.** Serves every org under its data root at
  `/org/<slug>/…`, the way the cluster pod does. Use it when the node *is*
  an org's host (`TASK_SERVER_ORG` picks one). Everything below works for
  either; the service files take `--host`.

## Get it

Each release `vX.Y.Z` (https://github.com/FastTrackStudios/task/releases)
carries standalone binaries, built by `.github/workflows/node-binaries.yml`
once production runs the release:

| Asset | Runs on |
|---|---|
| `task-server-linux-x86_64` | Linux x86-64, glibc ≥ 2.35 (Ubuntu 22.04, Debian 12, Fedora 36 and later) |
| `task-server-linux-aarch64` | Linux ARM64, glibc ≥ 2.35 (Raspberry Pi 4/5 on a 64-bit OS, Graviton, …) |
| `task-server-macos-aarch64` | macOS 14+ on Apple silicon |
| `SHA256SUMS` | the checksums of the three, `sha256sum` format |
| `SHA256SUMS.minisig` | a [minisign](https://jedisct1.github.io/minisign/) signature of `SHA256SUMS` by the release key, trusted comment `task-server v<X.Y.Z> SHA256SUMS` |

OpenSSL is linked in; the only runtime dependency is the system's libc.

The release key (the minisign public key; `RELEASE_SIGNING_KEY` in
`apps/server/src/self_update.rs`, pinned from v0.2.5 on):

```
RWRh9Gk9qaStQMhku4THMidEYrQsmAEsm2dDprUkZxShEtqfIcroM8Od
```

**Install script** (non-Nix hosts): picks the newest release for this
platform, checks `SHA256SUMS.minisig` against the release key (pinned in
the script) when `minisign` is on PATH, checks the sha256 and that the
binary reports that version, and installs `task-server` into
`/usr/local/bin` (as root) or `~/.local/bin`.
Without `minisign` the sha256 only proves the download is intact, not who
built it: that first install trusts GitHub and the copy of the script you
fetched from it (trust on first use), and the script says so. Every
`task-server self-update` after it checks the signature. Install
[minisign](https://jedisct1.github.io/minisign/) first to check the first
install too. Releases from before signing (v0.2.4 and older) carry no
signature; with `minisign` installed the script refuses them unless you
pass `--unsigned`:

```bash
curl -fsSL https://raw.githubusercontent.com/FastTrackStudios/task/main/scripts/install-node.sh | sh
# or a given release / place:
sh scripts/install-node.sh --version 0.2.5 --prefix /opt/task
```

By hand:

```bash
v=0.2.5; a=task-server-linux-x86_64
curl -fLO https://github.com/FastTrackStudios/task/releases/download/v$v/$a
curl -fLO https://github.com/FastTrackStudios/task/releases/download/v$v/SHA256SUMS
curl -fLO https://github.com/FastTrackStudios/task/releases/download/v$v/SHA256SUMS.minisig
minisign -V -P RWRh9Gk9qaStQMhku4THMidEYrQsmAEsm2dDprUkZxShEtqfIcroM8Od -m SHA256SUMS
# must print: Trusted comment: task-server v$v SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS      # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
install -m 755 $a /usr/local/bin/task-server && task-server --version
```

**Nix:** `nix run github:FastTrackStudios/task/v0.2.3#task-server --
--version`, or the NixOS module below.

## Run it as a service

### Linux (systemd)

The binary renders its own unit, so the paths in it are the ones on this
machine; the defaults are committed in `deploy/node/systemd/` (a test keeps
them identical to what the binary renders).

```bash
sudo task-server service write /etc/systemd/system     # task-server.service + the daily update (refuses to overwrite; --force)
# create the node's user and data root, then sign it in (a code to approve on your phone):
sudo useradd --system --home-dir /var/lib/task-server --shell /usr/sbin/nologin task
sudo install -d -o task -g task -m 0700 /var/lib/task-server
sudo -u task env TASK_DATA_ROOT=/var/lib/task-server task-server peer sign-in
sudo systemctl daemon-reload
sudo systemctl enable --now task-server.service
journalctl -u task-server -f
# the daily update, once your binary pins a release key (see "Updating"):
sudo systemctl enable --now task-server-update.timer
```

`task-server service write` takes `--binary`, `--data-root`, `--bind`,
`--user` and `--host` (`task-server service` lists them). The unit runs
`task-server peer` as `task`, with `TASK_DATA_ROOT=/var/lib/task-server`
(a systemd `StateDirectory`) and `TASK_SERVER_BIND=0.0.0.0:9090`, under
`ProtectSystem=strict` with only the data root writable. The update unit
runs as root (it replaces a root-owned binary and restarts the node), and
is sandboxed the same way: only the binary's directory is writable, no
new privileges, a private `/tmp`, and a 45-minute `TimeoutStartSec`.
`service write` refuses to replace a file that already exists (a unit you
edited stays yours); `--force` replaces them.

### macOS (launchd)

A per-user agent — no admin rights, and the binary in `~/.local/bin` so
updates need none either. Defaults in `deploy/node/launchd/` (with
`/Users/you` for your home; `service write` fills in the real one).

```bash
sh scripts/install-node.sh                                  # ~/.local/bin/task-server
TASK_DATA_ROOT="$HOME/Library/Application Support/task-server" ~/.local/bin/task-server peer sign-in
~/.local/bin/task-server service write ~/Library/LaunchAgents
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.fasttrackstudio.task-server.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.fasttrackstudio.task-server.update.plist
tail -f ~/Library/Logs/task-server.log
```

On a Mac that also runs the Task desktop app, a headless node is a second
device of your account with its own data root — it works, but the app's
own node already syncs while the app is open; the headless node is for a
Mac that stays on (a mini under the desk).

### NixOS

The flake exports the module; `package` defaults to the same flake's
`task-server`, so the node runs exactly the release the input is pinned
to:

```nix
# flake.nix
inputs.task.url = "github:FastTrackStudios/task/v0.2.3";

# the host
imports = [ inputs.task.nixosModules.task-server ];
services.task-server = {
  enable = true;
  # mode = "host";         # host orgs instead of running as your device
  # port = 9090;  openFirewall = true;  peerName = "battleship-node";
  # environmentFile = config.sops.secrets."task-node.env".path;
};
```

Then once: `sudo task-server-node peer sign-in` (the module ships
`task-server-node`, which runs `task-server` as the service's user against
the service's data root; `task-server-node peer status`, `… admin …`
likewise). A den-based fleet wraps the module in one line:
`den.aspects.task-node.nixos = inputs.task.nixosModules.task-server;`.
`nix build .#checks.x86_64-linux.nixos-task-server` evaluates the module
and checks the unit it renders.

## Data, network, pairing

- **Data root** (`TASK_DATA_ROOT`): everything the node holds — `orgs/<slug>/`
  (each org's stores, vault and peer identity), the node's install id,
  and `peer-account.json`, the account's session (owner-only, 0600). Back
  it up like any other device's data; it is the only state.
- **Network:** the node serves HTTP/WebSocket on `TASK_SERVER_BIND`
  (default `0.0.0.0:9090`) — the CLI, the web UI and other apps on your
  network can use it. Device-to-device sync runs over iroh (QUIC/UDP),
  which dials out and traverses NAT on its own, so a node behind a home
  router needs no port forwarding to sync with your phone. Open the TCP
  port only if something should reach the HTTP side directly.
- **Pairing — no central server, just another node.** `task-server peer
  sign-in` prints a URL and a code; approve it on a phone that is signed
  in. From then on the node runs the account pass every five minutes: it
  registers itself (name, platform `server`, endpoint id) in the account's
  device address book at the issuer — which stores names and endpoint ids,
  never content — joins the account's shared org from whichever of your
  devices holds it, and admits every other device's endpoint. Your phone
  and laptop admit it the same way on their next pass. After that the
  devices sync with each other directly; the node is only special in that
  it is usually reachable. `task-server peer status` says who it is signed
  in as and which orgs it holds. `TASK_PEER_NAME` sets the name your
  devices see (default: the hostname).

## Updates

The node never updates itself unasked; each way below checks the binary
before it replaces anything, and a failed check leaves the installed one
as it was.

- **Nix hosts follow the release tag.** The node's version is the `task`
  flake input's tag. The fleet moves that pin to the newest `v*` tag (the
  same release follower that keeps the desktop app current on NixOS — a
  scheduled bump of `inputs.task` to the newest `v*` tag, then the host's
  rebuild) and the switch restarts the node, because its `ExecStart`
  changes with the package. Rolling back is `nixos-rebuild --rollback`.
  `task-server self-update` refuses to touch a binary in `/nix/store`.
- **Everywhere else, `task-server self-update`.** It asks the GitHub
  releases API for the newest `vX.Y.Z` release (not drafts or
  prereleases) that carries this platform's asset and
  `SHA256SUMS`; checks `SHA256SUMS.minisig` is a signature of
  `SHA256SUMS` by the release key pinned in the binary
  (`RELEASE_SIGNING_KEY`, apps/server/src/self_update.rs) whose trusted
  comment names that release (so a signature cannot be lifted from an
  older one); streams the binary into a fresh, randomly named file beside
  the installed one while hashing it; checks its sha256 against
  `SHA256SUMS`; runs the download with `--version` and requires it to say
  `task-server X.Y.Z`; and renames it over the running binary (atomic; the
  running node keeps the old one until it restarts). `--check` only
  reports, `--to X.Y.Z` installs (or rolls back to) a given release, and
  `--then <command…>` runs a command only if a new binary was installed —
  the service files use it to restart the node:
  - systemd: `task-server-update.timer` (daily, randomised) runs
    `task-server self-update --then systemctl try-restart task-server.service`;
  - launchd: `app.fasttrackstudio.task-server.update` (daily and at login)
    runs `task-server self-update --then … launchctl kickstart -k …`.
  The same logic the desktop updater uses for its own assets on the same
  release (apps/desktop/src/updater.rs): newest release by version, verify the
  checksum, verify the thing itself, swap only then.
  **Why the signature:** the binary and `SHA256SUMS` come from the same
  release, so the checksum alone only proves the download is intact.
  Anyone who can write a release's assets could replace both, and the
  update runs as root on every node. The minisign secret key lives
  outside the release (the `TASK_RELEASE_MINISIGN_KEY` Actions secret,
  used by `node-binaries.yml`, which also refuses to sign with a key the
  source does not pin, and fails rather than publish unsigned binaries for
  a tag that pins one). A binary built while `RELEASE_SIGNING_KEY` was
  still `None` (v0.2.4 and older) installs nothing and says so, and so
  does an unsigned release: move such a node on with the install script.
- **By hand:** run `scripts/install-node.sh` again; it is a no-op when the
  newest release is already installed.

## Versions

One version everywhere: `[workspace.package] version` in the root
Cargo.toml is what `task-server --version`, `task --version`,
`fts-files-daemon --version`, `task-app-desktop --version`, the iOS and
macOS apps' marketing version, the image tag `v<version>`, the GitHub
Release `v<version>` and the Nix packages all report.
`scripts/check-versions.sh` (`just ci-versions`, part of `just ci` and the
release checks) fails if any of them takes its version from anywhere
else; the deploy and `node-binaries.yml` run it with `--bin` on the
binaries they built, so a binary that *reports* something else never
ships.

## On the name

The Captain's framing — and `offline.peers` — is that this is a node, not
a server: the "server" is just a node that stays on. The binary keeps the
name `task-server` for now, because the image, the chart, the pod, the
admin commands, `self-update`'s asset names and every runbook say
`task-server`, and renaming them is a migration of its own. **Proposed:**
ship the same binary as `task-node` (a second name for one binary, e.g. a
`[[bin]]` alias or a symlink in the release), lead the docs with
`task-node peer`, and retire `task-server` as the command name over a few
releases. Not done here; it wants the Captain's yes.
