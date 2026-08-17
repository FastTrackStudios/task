
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
