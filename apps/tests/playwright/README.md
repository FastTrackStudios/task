# Browser tests (Playwright)

Smoke coverage for the Task web app (`apps/web`). Modeled on the sibling
`Editor/tests/` setup and the Dioxus repo's own
`packages/playwright-tests/` — a single `playwright.config.js` whose
`webServer` boots `dx serve` (web platform) for the run.

Everything is **self-contained in the flake**: the `playwright` dev
shell (`flake.nix` → `devShells.playwright`) provides `nodejs`, `pnpm`,
and a Nix-managed Chromium via `playwright-driver.browsers`, with
`PLAYWRIGHT_BROWSERS_PATH` preset — so you never run
`playwright install`.

## Layout

```
tests/playwright/
├── package.json          # @playwright/test (pinned to nix playwright-driver)
├── playwright.config.js   # workers=1, single dx-serve webServer
├── smoke.spec.js          # shell + route smoke + screenshots
└── README.md              # you are here
```

## Run

From the repo root:

```sh
nix develop .#playwright
cd tests/playwright
pnpm install            # fetches @playwright/test only (no browser download)
pnpm test               # headless; HTML report on failure
```

Other modes:

```sh
pnpm test:headed        # watch it drive a real browser
pnpm test:ui            # Playwright's interactive UI
pnpm report             # open the last HTML report
```

The first run includes a one-time wasm build (the `webServer` boot has
a 10-minute timeout for that). Subsequent runs reuse the build.

Override the port if 9100 is taken: `PW_PORT=9200 pnpm test`.

## What it checks

`smoke.spec.js` loads each primary route (`/`, `/tasks`, `/projects`,
`/goals`, `/gantt`), asserts the Dioxus shell mounted (sidebar nav
renders), and writes a full-page screenshot of each to `screenshots/`
(gitignored) — so you can eyeball the running app. These are
shell-level assertions, so they pass **with or without** a live
`task-server`: data-backed pages render their loading/empty/error
state when the vox endpoint is absent.

## Version pinning (important)

`@playwright/test` in `package.json` **must match the flake's locked
`playwright-driver`** (NOT your registry's `nixpkgs#…`, which is often
newer and ships different browser revisions → "Executable doesn't
exist" errors). Query the flake's actual version and bump to match:

```sh
nix eval --raw --impure --expr \
  '(builtins.getFlake (toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem}.playwright-driver.version'
# currently 1.58.2  (ships chromium-1208)
```

After bumping nixpkgs in `flake.lock`, re-check this and re-pin.

## Data-driven tests (opt-in, future)

To exercise live tasks/projects, run a seeded `task-server` on the port
baked into `DEFAULT_VOX_URL` (`ws://127.0.0.1:18080/vox`, see
`crates/ui/src/vox_session.rs`) and build the web app with
`TASK_VOX_URL_WEB` pointing at it, then add a second `webServer` entry
to the config.
