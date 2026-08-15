// @ts-check
// Coverage for the behaviors that are BROWSER-ONLY by design — the
// things the shared Rust core deliberately leaves to contenteditable
// and therefore can never be exercised by the native (dioxus-test)
// suites: clipboard paste, IME composition, and the wrap-aware
// visual-line hjkl (`Selection.modify`) vim shortcut.
//
// Same conventions as editor.spec.js: `?seed=` URL seeding, the
// `[data-editor-id]` target, and the `#dbg-*` debug-panel mirror.

const { test, expect } = require("@playwright/test");

const editor = (page) => page.locator("[data-editor-id]").first();

async function readState(page) {
  return {
    len: await page.locator("#dbg-len").textContent(),
    anchor: await page.locator("#dbg-anchor").textContent(),
    head: await page.locator("#dbg-head").textContent(),
    text: await page.locator("#dbg-text").textContent(),
  };
}

async function waitForLen(page, n) {
  await expect(page.locator("#dbg-len")).toHaveText(String(n));
}

/** Navigate to the playground with `text` as the whole doc. */
async function open(page, text, { vim = false } = {}) {
  const seed = encodeURIComponent(text);
  const url = vim ? `/?seed=${seed}` : `/?seed=${seed}&novim=1`;
  await page.goto(url);
  await editor(page).waitFor();
  await waitForLen(page, text.length);
  await editor(page).focus();
}

/** Put the caret at the very end of the (single-line-aware) doc. */
async function caretToEnd(page) {
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("ArrowRight");
}

test.describe("browser-only behaviors", () => {
  test.beforeEach(async ({ page }) => {
    page.on("pageerror", (err) => {
      throw err;
    });
  });

  test("clipboard paste lands in the doc through the input bridge", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page, "ab");
    await caretToEnd(page);
    await page.evaluate(() => navigator.clipboard.writeText("PASTED"));
    await page.keyboard.press("ControlOrMeta+v");
    await waitForLen(page, "abPASTED".length);
    const state = await readState(page);
    expect(state.text).toBe("abPASTED");
    expect(Number(state.head)).toBe("abPASTED".length);
  });

  test("multi-line paste preserves newlines", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page, "x");
    await caretToEnd(page);
    await page.evaluate(() => navigator.clipboard.writeText("one\ntwo"));
    await page.keyboard.press("ControlOrMeta+v");
    await waitForLen(page, "xone\ntwo".length);
    const state = await readState(page);
    expect(state.text).toBe("xone\ntwo");
  });

  test("IME-style composition commits once without duplication", async ({
    page,
  }) => {
    await open(page, "x");
    await caretToEnd(page);
    // Approximate an IME session: compositionstart → text lands in the
    // DOM (insertText stands in for insertCompositionText, which can't
    // be synthesized as a trusted event) → compositionend. The bridge
    // must hold updates while composing and commit exactly once at the
    // end — no dropped text, no double-apply.
    await page.evaluate(() => {
      const el = document.querySelector("[data-editor-id]");
      el.dispatchEvent(
        new CompositionEvent("compositionstart", { bubbles: true })
      );
    });
    await page.keyboard.insertText("かな");
    await page.evaluate(() => {
      const el = document.querySelector("[data-editor-id]");
      el.dispatchEvent(
        new CompositionEvent("compositionend", { bubbles: true, data: "かな" })
      );
    });
    // #dbg-len counts BYTES (Rust side), not JS UTF-16 units:
    // "xかな" = 1 + 3 + 3 bytes.
    await waitForLen(page, 7);
    const state = await readState(page);
    expect(state.text).toBe("xかな");
  });

  test("holding k at the top never drops into insert mode", async ({
    page,
  }) => {
    // Regression: with a frontmatter properties widget at the top,
    // repeated `k` used to walk the DOM selection into a property
    // cell (via the visual-arrow Selection.modify shortcut), the
    // cell's focus flipped vim to Insert, and further `k`s typed
    // "kkkk" into the doc.
    const doc = "---\ntitle: t\ntags: []\n---\nbody line";
    await open(page, doc, { vim: true });
    // Down to the body line, then hammer `k` well past the top.
    await page.keyboard.press("G");
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press("k");
    }
    // Still in Normal mode…
    await expect(page.locator(".vim-mode")).toHaveText("NORMAL");
    // …and more `k`s must not type anything.
    await page.keyboard.press("k");
    await page.keyboard.press("k");
    await page.keyboard.press("k");
    const state = await readState(page);
    expect(state.text).not.toContain("kk");
    expect(state.text).toBe(doc);
    await expect(page.locator(".vim-mode")).toHaveText("NORMAL");
  });

  test("vim j moves by VISUAL row on a soft-wrapped line", async ({
    page,
  }) => {
    // Narrow viewport so one long logical line wraps into several
    // visual rows. Vim default-on (no ?novim).
    await page.setViewportSize({ width: 480, height: 800 });
    const long =
      "alpha beta gamma delta epsilon zeta eta theta iota kappa " +
      "lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
    await open(page, long, { vim: true });
    // Vim starts in Normal mode; go to line start.
    await page.keyboard.press("0");
    await expect(page.locator("#dbg-head")).toHaveText("0");

    // `j` must move DOWN one rendered row — i.e. somewhere strictly
    // inside the same logical line, not past its end (there is no
    // second logical line to land on).
    await page.keyboard.press("j");
    await expect
      .poll(async () => Number((await readState(page)).head))
      .toBeGreaterThan(0);
    const head = Number((await readState(page)).head);
    expect(head).toBeLessThan(long.length);
  });

  test("vim j/k round-trips across visual rows", async ({ page }) => {
    await page.setViewportSize({ width: 480, height: 800 });
    const long =
      "one two three four five six seven eight nine ten eleven twelve " +
      "thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
    await open(page, long, { vim: true });
    // Start at column 3 — a wrap boundary offset is ambiguous (end of
    // row N == start of row N+1), so a col-0 round-trip can stall on
    // selection affinity. Goal-column restores col 3 exactly.
    await page.keyboard.press("0");
    await page.keyboard.press("l");
    await page.keyboard.press("l");
    await page.keyboard.press("l");
    await expect(page.locator("#dbg-head")).toHaveText("3");

    await page.keyboard.press("j");
    await expect
      .poll(async () => Number((await readState(page)).head))
      .toBeGreaterThan(3);
    await page.keyboard.press("k");
    await expect(page.locator("#dbg-head")).toHaveText("3");
  });
});
