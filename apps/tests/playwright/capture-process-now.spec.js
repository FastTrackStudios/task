// @ts-check
const { test, expect } = require("@playwright/test");

/**
 * The global capture door can process on the spot
 * (`process.capture.process-now`, docs/spec/capture-and-process.md).
 *
 * The ✎ Capture button opens the one capture surface (the composer, as
 * a Capture). While what is typed is still a plain capture it offers
 * "Process now", which hands the words to the same process card the
 * Inbox deck uses — every outcome on it — instead of only filing them.
 * Closing the card undecided files the capture in the inbox, so nothing
 * typed is lost. Once the words carry something a task has (`!high`),
 * the surface is making a task and the door is not offered.
 *
 * The shell only mounts once org discovery answers, so this needs the
 * example studio served and the web app pointed at it:
 *
 *   just demo serve                      # ACME on :18080
 *   just demo web                        # web app on :8766, signed in as Alice
 *   PW_PORT=8766 pnpm exec playwright test capture-process-now.spec.js
 *
 * It asserts the door and the card only; the writes a decision makes
 * are covered over the wire by tests/integration/tests/it/process.rs.
 * Screenshots land in `screenshots/` (gitignored).
 */

const SHELL_TIMEOUT = 45_000;
const WORDS = "Ask the piano tuner what the cracked soundboard costs";

async function openCapture(page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  const door = page.getByTitle("Capture a fleeting note").first();
  await expect(door).toBeVisible({ timeout: SHELL_TIMEOUT });
  await door.click();
  await expect(page.getByTestId("composer-kind")).toHaveText("Capture");
}

test("the capture door processes what was just captured", async ({ page }) => {
  await openCapture(page);
  await page.locator("#composer-title").fill(WORDS);

  const processNow = page.getByTestId("composer-process");
  await expect(processNow).toBeVisible();
  await page.screenshot({ path: "screenshots/capture-door.png" });
  await processNow.click();

  // The composer gives way to the process card, holding the words verbatim.
  const card = page.getByTestId("process-now");
  await expect(card).toBeVisible();
  await expect(page.getByTestId("composer-kind")).toHaveCount(0);
  await expect(card.getByText(WORDS, { exact: true })).toBeVisible();
  for (const outcome of [
    "Do it now",
    "Add to a project",
    "Write a note",
    "Snooze",
    "Keep as reference",
    "It's a project",
    "Already done",
    "Delete",
  ]) {
    await expect(card.getByText(outcome, { exact: true })).toBeVisible();
  }
  await page.screenshot({ path: "screenshots/process-now-card.png" });

  // Each step past the first steps back to it.
  await card.getByText("Snooze", { exact: true }).click();
  await expect(card.getByText("When should it come back?")).toBeVisible();
  for (const when of ["Soon", "Maybe", "Someday"]) {
    await expect(card.getByText(when, { exact: true })).toBeVisible();
  }
  await card.getByText("‹ Back").click();
  await expect(card.getByText("Do it now", { exact: true })).toBeVisible();

  // Closing undecided keeps it: filed in the inbox, open.
  await card.getByLabel("Keep in inbox").click();
  await expect(card).toHaveCount(0);
  await expect(page.getByText("In your inbox")).toBeVisible();
});

test("a capture given what a task has is a task, with no process door", async ({ page }) => {
  await openCapture(page);
  await page.locator("#composer-title").fill(`${WORDS} !high`);
  await expect(page.getByTestId("composer-kind")).toHaveText("Task");
  await expect(page.getByTestId("composer-process")).toHaveCount(0);
});
