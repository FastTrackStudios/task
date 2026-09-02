// @ts-check
const { test, expect } = require("@playwright/test");

/**
 * Vault editing end-to-end: create a note, type into it, link to another
 * note, and prove it all survived a reload.
 *
 * This is the CRDT path, exercised the way a person exercises it. Note
 * bodies do not go over a plain "save" RPC — every keystroke becomes a
 * Loro transaction that `crates/ui/src/collab.rs` folds into the shared
 * replica and the `doc-sync/sync` session streams to the server. So the
 * reload assertion is the real one: text still on screen after a fresh
 * page load means the edits actually round-tripped through doc-sync and
 * were persisted, not just echoed into the local editor.
 *
 * It also carries the same connection-churn guards as
 * `connection-stability.spec.js`, because typing is exactly when the
 * doc-sync path is loud enough to trip vox's 64-live-request limit (see
 * that spec's header for what that failure looks like).
 *
 * Requires a running `task-server` on the endpoint baked into the build
 * (`just demo serve`) — it skips when the editor never mounts, so the
 * shell-only smoke run stays green.
 */

const PROSE = "The quick brown fox jumps over the lazy dog.";
const LINK_TARGET = "Linked Target Note";

/** The web editor root (`editor-view`'s contenteditable). */
const EDITOR = '[data-editor-id][contenteditable="plaintext-only"]';

/** Console signatures of the reconnect storm — see connection-stability.spec.js. */
const PROTOCOL_ERROR = /received protocol error from peer/i;
const OVER_LIMIT = /max_concurrent_requests exceeded/i;
const CONNECTION_LOST = /connection lost; reconnecting/i;
const FATAL = /closure invoked recursively or after being dropped|unreachable executed/i;

/**
 * Put the editor into a state where typed characters are text.
 *
 * The editor is modal when `UserPrefs::vim_mode` is on (default off).
 * The root element carries the mode as a class (`vim-mode-normal` /
 * `vim-mode-insert`), so this is a no-op unless we're actually sitting
 * in normal mode — pressing `i` while already inserting would type a
 * literal "i" into the note.
 *
 * @param {import('@playwright/test').Page} page
 * @param {import('@playwright/test').Locator} editor
 * @param {{via?: string}} [opts] key to enter insert with (`i`, or `o`
 *   to open a line below first)
 */
async function enterInsertMode(page, editor, opts = {}) {
  const cls = (await editor.getAttribute("class")) ?? "";
  if (!cls.includes("vim-mode-normal")) return;
  await page.keyboard.press(opts.via ?? "i");
  await expect(editor).toHaveClass(/vim-mode-insert/, { timeout: 5_000 });
}

test("a new vault note takes typing and a wikilink, and survives a reload", async ({
  page,
}) => {
  const lines = [];
  const fatal = [];
  page.on("console", (m) => lines.push(m.text()));
  page.on("pageerror", (e) => {
    if (FATAL.test(String(e))) fatal.push(String(e));
  });

  await page.goto("/vault", { waitUntil: "domcontentloaded" });

  // ── create ────────────────────────────────────────────────────────
  // Via the command palette, which is the path a person actually takes
  // (and the one the shell logs as `fts.palette.toggle` →
  // `fts.task.new_note`). The vault tree's "New note…" field is not
  // usable here: it stays in the DOM but hidden while the tree pane is
  // collapsed, and that collapse state is per browser profile.
  // Wait for the shell to actually be up first: the palette is a
  // keyboard chord, and a chord pressed against a still-booting wasm
  // app is simply lost — there is no element to retry against.
  await page
    .getByPlaceholder("Start a timer…")
    .waitFor({ state: "visible", timeout: 90_000 })
    .catch(() => {});

  const palette = page.getByPlaceholder(/Run a command, jump to a page/i);
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.keyboard.press("Control+k");
    if (await palette.waitFor({ state: "visible", timeout: 15_000 }).then(
      () => true,
      () => false,
    )) {
      break;
    }
  }
  test.skip(
    !(await palette.isVisible()),
    "command palette never opened — is a task-server running on the build's vox endpoint?",
  );

  // The action's registered name (`fts.task.new_note` → "Create new
  // Note"). Typing the full name rather than a loose query keeps the
  // top hit deterministic — the palette also searches notes, so "new
  // note" can rank an existing file first.
  await page.keyboard.type("Create new Note", { delay: 30 });
  await page.waitForTimeout(1_000);
  await page.keyboard.press("Enter");

  // Creation is a round trip (pick a free `Untitled N.md`, create it,
  // then navigate), so wait on the route before the editor.
  await expect(page).toHaveURL(/[?&]path=[^&]*\.md/, { timeout: 45_000 });
  const notePath = page.url();

  const editor = page.locator(EDITOR).first();
  await expect(editor).toBeVisible({ timeout: 30_000 });
  // `spawn_new_note` parks the fresh path in `PendingTitleEdit`, so the
  // note opens with the TITLE in edit mode and its text selected —
  // typing now would rename the file instead of writing the body.
  await page.keyboard.press("Escape");

  // ── type ──────────────────────────────────────────────────────────
  // A seeded note opens with a frontmatter block, which the editor
  // renders as an interactive `.md-properties` widget (created / tags /
  // aliases). Clicking lands the caret wherever the click was, so the
  // first keystrokes can disappear into that widget — jump to the end
  // of the document before typing any body text.
  // Click the BOTTOM of the editor box, not its centre: the widget
  // occupies the top of the document and a centre click can land the
  // caret inside it, where the first keystrokes vanish into a property
  // field. (`Control+End` does not help — this editor has its own
  // keymap and does not bind it.)
  const box = await editor.boundingBox();
  if (!box) throw new Error("editor has no layout box");
  await page.mouse.click(box.x + 40, box.y + box.height - 16);
  await page.waitForTimeout(500);

  // Modal editing is opt-in and defaults OFF (`UserPrefs::vim_mode`),
  // so this is normally a no-op — but an account that turned vim ON
  // opens the note in NORMAL mode, where letters are commands rather
  // than text (typing "The qui" runs T,h / e / q,u and only the `i`
  // starts inserting). Enter insert mode only when we're actually in
  // normal mode; pressing `i` while already inserting would type a
  // literal "i".
  await enterInsertMode(page, editor);

  await page.keyboard.type(PROSE, { delay: 15 });
  await page.keyboard.press("Enter");

  // ── link ──────────────────────────────────────────────────────────
  // `[[` opens the wikilink completion menu; type the whole link in one
  // go and dismiss the menu after.
  await page.keyboard.type(`[[${LINK_TARGET}]]`, { delay: 15 });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(300);
  // A wikilink only renders as a chip while the caret is OFF it
  // (`markdown.rs`: `cursor_touches` keeps the raw source visible), so
  // step the caret away before asserting on the decoration. In vim,
  // Escape above dropped us to normal mode and `o` opens a line below;
  // without vim we're still inserting and Enter breaks the line.
  await enterInsertMode(page, editor, { via: "o" });
  await page.keyboard.press("Enter");
  await page.waitForTimeout(300);

  await expect(editor).toContainText(PROSE, { timeout: 10_000 });
  await expect(
    page.locator(".md-wikilink", { hasText: LINK_TARGET }).first(),
  ).toBeVisible({ timeout: 10_000 });

  await page.screenshot({ path: "screenshots/vault-editing.png", fullPage: true });

  // ── persist ───────────────────────────────────────────────────────
  // Give the sync session a beat to flush the trailing batch, then
  // prove the server has it by coming back on a cold page.
  await page.waitForTimeout(3_000);
  const settled = lines.length;

  await page.goto(notePath, { waitUntil: "domcontentloaded" });
  const reloaded = page.locator(EDITOR).first();
  await expect(reloaded).toBeVisible({ timeout: 45_000 });
  await expect(reloaded).toContainText(PROSE, { timeout: 30_000 });
  await expect(reloaded).toContainText(LINK_TARGET, { timeout: 10_000 });

  // ── the connection survived all of it ─────────────────────────────
  const after = lines.slice(settled);
  const count = (re) => after.filter((l) => re.test(l)).length;
  const context = after
    .filter((l) => CONNECTION_LOST.test(l) || PROTOCOL_ERROR.test(l) || OVER_LIMIT.test(l))
    .slice(0, 10)
    .join("\n");

  expect(count(PROTOCOL_ERROR), `protocol errors while editing:\n${context}`).toBe(0);
  expect(count(OVER_LIMIT), `concurrency limit exceeded while editing:\n${context}`).toBe(0);
  // One reconnect is the reload itself; more than that is churn.
  expect(
    count(CONNECTION_LOST),
    `reconnect churn while editing:\n${context}`,
  ).toBeLessThanOrEqual(2);
  expect(fatal, `fatal wasm/WS errors:\n${fatal.join("\n")}`).toHaveLength(0);
});
