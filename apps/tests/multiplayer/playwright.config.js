// @ts-check
// Playwright config for the multiplayer conformance suites
// (tracked issue dd824506).
//
// Unlike `tests/playwright/` (smoke, one dx-serve webServer) this
// harness owns its whole stack in global-setup:
//   - an ISOLATED `task-server` on MP_SERVER_PORT (default 18091)
//     with a throwaway TASK_DATA_ROOT seeded via the CLI,
//   - a static SPA server on MP_WEB_PORT (default 18092) hosting the
//     dx-built wasm bundle (built with TASK_VOX_URL_WEB baked to the
//     test server port — see run.sh).
//
// NEVER points at the dev server on :18080. Build + run:
//   nix develop --command ./run.sh
//
// The suites are real-time conformance checks: retries are OFF on
// purpose (a flake IS a finding); convergence waits use bounded
// `settle()` helpers, not sleeps.

const { defineConfig, devices } = require("@playwright/test");

const WEB_PORT = parseInt(process.env.MP_WEB_PORT || "18092", 10);

module.exports = defineConfig({
  testDir: ".",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"], ["html", { open: "never" }]],
  // Per-suite budgets are set with test.setTimeout() in the specs
  // (the churn suite alone runs ~4 min); this is the default.
  timeout: 180_000,
  expect: { timeout: 15_000 },
  globalSetup: "./global-setup.js",
  globalTeardown: "./global-teardown.js",
  use: {
    baseURL: `http://127.0.0.1:${WEB_PORT}`,
    // Traces for 25 contexts are enormous; artifacts.js dumps the
    // useful state (per-peer doc text, roster, console, vox state)
    // on failure instead.
    trace: "off",
    screenshot: "off",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
