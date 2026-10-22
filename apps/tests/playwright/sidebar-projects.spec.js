// @ts-check
const { test, expect } = require("@playwright/test");

/**
 * The sidebar's Projects section lists a few recent TOP-LEVEL projects,
 * never subprojects, and ends in "All projects →"
 * (`crates/ui/src/recent_projects.rs`).
 *
 * The example studio has an album with a song that is a project of its
 * own: `Example Album` and its subproject `Track Two` (parentId set by
 * the seeder). The sidebar must show the album once and the song not at
 * all; the Projects page keeps the song.
 *
 * The shell only mounts once org discovery answers, so this needs the
 * example studio served and the web app pointed at it:
 *
 *   just demo serve                      # ACME on :18080
 *   just demo web                        # web app on :8766, signed in as Alice
 *   PW_PORT=8766 pnpm exec playwright test sidebar-projects.spec.js
 *
 * Screenshots land in `screenshots/` (gitignored).
 */

const SHELL_TIMEOUT = 45_000;
const ALBUM = "Example Album";
const SONG = "Track Two";

test("the sidebar lists the album once and none of its songs", async ({ page }) => {
  await page.goto("/tasks", { waitUntil: "domcontentloaded" });
  const list = page.getByTestId("sidebar-projects").first();
  await expect(list).toBeVisible({ timeout: SHELL_TIMEOUT });
  const rows = list.getByTestId("sidebar-project");
  await expect(rows.filter({ hasText: ALBUM })).toHaveCount(1, { timeout: SHELL_TIMEOUT });
  await expect(rows.filter({ hasText: SONG })).toHaveCount(0);
  expect(await rows.count()).toBeLessThanOrEqual(7);
  await page.screenshot({ path: "screenshots/sidebar-projects.png" });

  // "All projects →" opens the full list, where the song still lives.
  const all = list.getByTestId("sidebar-all-projects");
  await expect(all).toBeVisible();
  await all.click();
  await expect(page).toHaveURL(/\/projects$/);
  await expect(page.getByText(SONG).first()).toBeVisible({ timeout: SHELL_TIMEOUT });
});

test("opening a song lights up its album, not the song", async ({ page }) => {
  await page.goto("/projects", { waitUntil: "domcontentloaded" });
  const song = page.locator('a[href^="/projects/"]', { hasText: SONG }).first();
  await expect(song).toBeVisible({ timeout: SHELL_TIMEOUT });
  await song.click();
  await expect(page).toHaveURL(/\/projects\/[0-9a-f-]{36}$/);

  await page.goto("/tasks", { waitUntil: "domcontentloaded" });
  const rows = page.getByTestId("sidebar-projects").first().getByTestId("sidebar-project");
  await expect(rows.first()).toBeVisible({ timeout: SHELL_TIMEOUT });
  await expect(rows.filter({ hasText: SONG })).toHaveCount(0);
  await expect(rows.first()).toContainText(ALBUM);
});
