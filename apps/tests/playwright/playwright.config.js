// @ts-check
const { defineConfig, devices } = require("@playwright/test");

/**
 * Playwright config for the Task web app (`apps/task/web`,
 * package `task-app-web`).
 *
 * Modeled on the sibling `Editor/tests/playwright.config.js` and the
 * Dioxus repo's own `packages/playwright-tests/`. A single `webServer`
 * boots `dx serve` (web platform) for the run; tests connect over
 * `baseURL`. The smoke suite asserts the shell + routes render, so it
 * does NOT require a running `task-server` — pages render their
 * loading/empty states when the vox endpoint is absent.
 *
 * Run (from the repo root):
 *   nix develop <repo-root>
 *   cd tests/playwright && pnpm install
 *   pnpm test            # or: pnpm test:headed / pnpm test:ui
 *
 * The dev shell pins Chromium via `playwright-driver.browsers`, so you
 * do NOT run `playwright install`.
 *
 * DATA-DRIVEN TESTS (opt-in): to exercise live tasks/projects, also run
 * a seeded `task-server` on the port baked into `DEFAULT_VOX_URL`
 * (`ws://127.0.0.1:18080/vox`, see `crates/task/ui/src/vox_session.rs`) and
 * build the web app with `TASK_VOX_URL_WEB` pointing at it. Add it as a
 * second `webServer` entry when those specs land.
 */

const PORT = parseInt(process.env.PW_PORT || "9100", 10);

module.exports = defineConfig({
  testDir: ".",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "list" : "html",
  // The first run includes a wasm build; keep per-test timeouts sane
  // (the slow part is the one-time server boot, handled below).
  timeout: 90_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    // Force the web platform and a fixed port. `dx serve` builds the
    // wasm bundle and hosts it. cwd is `apps/task/web` relative to
    // this config (apps/task/tests/playwright/).
    command: `dx serve --platform web --addr 127.0.0.1 --port ${PORT}`,
    cwd: "../../web",
    url: `http://127.0.0.1:${PORT}`,
    // First-time wasm build can be very slow.
    timeout: 10 * 60 * 1000,
    reuseExistingServer: !process.env.CI,
    stdout: "pipe",
    stderr: "pipe",
  },
});
