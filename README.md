
This Project is currently an experiment on the writings and videos by the incredible [No Boilerplate's Obsidian Series](https://www.youtube.com/watch?v=BTuGvfQGOrY&list=PLZaoyhMXgBzp9blLuIrwr5eRgBdnPy3BM).

There are a few main goals

- A Single Trusted System Upheld by The Temporal Contract
- A Local First, but Multiplayer Collaborative _realtime_ User Experience
- The Possibility for Federated Servers


Some Applications I intend to replace using Task in my own personal life
- Obsidian: My Dear Beloved, I adore obsidian, what I don't love is that I can't share my research about certain topics without also sharing my weekly mealplan schedule. Quartz Exists, but I can't contribute my Knowledge back into the world without filtering it first. Otherwise I have to treat everything in the vault as public, then its shareable or have some sort of export process.
- Samply: Love this project, use it all the time, just has different prioirities that what I need and I've been bitten by an update that has broken things for me or a client more than a handful of times.
- Frame.io: Expensive. I don't want to hold my video files hostage to a subscription service to have my content trained on AI.
- Nextcloud: Incredible project, enough usecases that I have differ that I need something custom, mainly audio and video playback, and some other crazy integration stuff I have in mind.
- Karpathy's LLM Wiki: Interesting Concept and Project, I'd like for it to be included in the system.

## Installation

Task has no central server. Every install is a **node** holding your data,
and your nodes sync with each other directly:

- the **desktop app** is a node with a window;
- **`task-server`** is the same node with no window. Leave it running on any
  computer (a desktop that stays on, a home server, a VPS) so one of your
  nodes is always reachable.

Sign each one in to your account and they find and admit each other. No
ports to forward, no addresses to type.

Every release `vX.Y.Z` on [GitHub Releases](https://github.com/FastTrackStudios/task/releases/latest)
carries everything from that version: the desktop apps, `task-server`, and
the web app.

### Desktop app

| Platform | Get it |
|---|---|
| macOS (Apple silicon) | [Task-macos.dmg](https://github.com/FastTrackStudios/task/releases/latest/download/Task-macos.dmg). Open it and drag Task to Applications |
| Linux (x86-64) | [Task-linux-x86_64.AppImage](https://github.com/FastTrackStudios/task/releases/latest/download/Task-linux-x86_64.AppImage). `chmod +x` it and run it |
| NixOS | `nix run github:FastTrackStudios/task#task-desktop` (prebuilt on `fasttrackstudio.cachix.org`) |

Installed apps update themselves: they check the latest release in the
background and install the update on the next start. The tray's *Check for
updates* does it right away.

### Headless server (an always-on node)

On macOS or Linux, with no Nix:

```sh
curl -fsSL https://raw.githubusercontent.com/FastTrackStudios/task/main/scripts/install-node.sh | sh
task-server peer sign-in     # approve the code on a phone that's signed in
task-server peer             # run it as one more of your devices
```

To keep it running across reboots, `task-server service write` sets up the
systemd unit or launchd agent, and a daily `task-server self-update`.
[docs/headless-node.md](docs/headless-node.md) has the service setup for
Linux, macOS and NixOS.

### Host orgs with the web app

To host orgs for a team and serve the web app in a browser, as
task.starcommand.live does, run the public images
(`ghcr.io/fasttrackstudios/task-server` and `task-web`). The compose file
runs the server, the web app, and a proxy that puts them on one origin:

```sh
curl -fsSLO https://raw.githubusercontent.com/FastTrackStudios/task/main/deploy/docker-compose.yml
docker compose up -d   # then open http://localhost:8090
```

There's also a Helm chart. [deploy/README.md](deploy/README.md) covers both,
and how to create your first org.

## Repository layout

One cargo workspace, rooted here. Five code directories, following the
`architect` layout conventions:

| Path | What lives there |
|---|---|
| `apps/` | The runtime products: `cli`, `server`, `web`, `desktop`, `mobile`, `watchos`. Plus the shared Tailwind input and the browser/multiplayer suites in `tests/`. |
| `crates/` | Standalone shared libraries — the UI shell, widgets, player, plugin vocabulary. |
| `features/` | Vertical capability slices, the bulk of the tree. Each is `<feature>/<feature>-proto` + a facade + optional `-db` / `-ui` / `-live` backends. |
| `examples/` | Reference consumers — `federation`, and a demo `vault`. |
| `xtask/` | Build tooling, run via `cargo xtask`. |

Supporting directories: `nix/` (dendritic flake modules under `modules/`,
plain NixOS modules under `nixos/`), `deploy/` (Helm chart + compose),
`docs/`, `skills/` (agent recipes), `scripts/` (ops shell scripts), and
`.githooks/`.

Run `just` for the recipe menu — `just server`, `just web`, `just dev`,
`just check`, `just ci`. 
