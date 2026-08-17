// @ts-check
// Browser tests for the Editor playground.
//
// Conventions:
// - The editor is the `[data-editor-id]` contenteditable. We
//   target it directly rather than via class so tests survive
//   styling churn.
// - State assertions read the debug panel's id-tagged
//   elements (`#dbg-len`, `#dbg-anchor`, `#dbg-head`,
//   `#dbg-text`). This sidesteps having to walk Dioxus's
//   rendered tile tree to reconstruct state — the debug panel
//   is already a faithful mirror of `EditorState`.
// - Helper `setSelection(...)` uses page.evaluate() to set the
//   DOM Selection directly, matching what a click or arrow
//   keypress would produce. We need this because Playwright
//   doesn't have a high-level "place caret in a contenteditable
//   at byte offset N" primitive.

const { test, expect } = require("@playwright/test");

/** Locator for the contenteditable editor element. */
const editor = (page) => page.locator("[data-editor-id]").first();

/**
 * Read the live `EditorState` mirror from the debug panel.
 * Returns `{ len, anchor, head, text }` — all strings as DOM
 * `textContent`. Tests `Number()`-cast as needed.
 */
async function readState(page) {
  return {
    len: await page.locator("#dbg-len").textContent(),
    anchor: await page.locator("#dbg-anchor").textContent(),
    head: await page.locator("#dbg-head").textContent(),
    text: await page.locator("#dbg-text").textContent(),
  };
}

/**
 * Place a caret inside the editor at the given visible-text
 * offset (counted by walking text descendants in order). For
 * Phase 8 (LineTile) the editor has multiple text nodes
 * across spans and lines, so a naive `setStart(editor, N)`
 * wouldn't work.
 */
async function setCaret(page, docOffset) {
  await page.evaluate((off) => {
    const el = document.querySelector("[data-editor-id]");
    const sel = window.getSelection();
    const range = document.createRange();
    // Place the caret at *doc offset* `off`, not visible-text
    // offset. We use the `data-tile-pos` attribute on rendered
    // tiles (Text/Mark/Widget/Line) — each carries its start
    // position in doc space, so we can locate the owning tile
    // by binary-friendly walk and place the range at the right
    // text-node offset inside.
    const tiles = Array.from(el.querySelectorAll("[data-tile-pos]"));
    const candidates = tiles
      .map((t) => {
        const pos = parseInt(t.dataset.tilePos, 10);
        const text = t.firstChild;
        const len =
          text && text.nodeType === 3 ? text.nodeValue.length : 0;
        return { tile: t, text, pos, end: pos + len };
      })
      .sort((a, b) => a.pos - b.pos);
    // Find the latest tile whose `[pos, end]` covers `off`.
    let chosen = null;
    for (const c of candidates) {
      if (c.pos <= off && off <= c.end) chosen = c;
    }
    if (chosen && chosen.text && chosen.text.nodeType === 3) {
      const local = off - chosen.pos;
      range.setStart(
        chosen.text,
        Math.min(local, chosen.text.nodeValue.length)
      );
    } else if (chosen) {
      // Empty tile (e.g., empty line whose only child is a
      // `<br>`). Anchor the range INSIDE the tile div at
      // offset 0 — that places the caret before the `<br>`,
      // which is what the browser uses for empty
      // contenteditable lines.
      range.setStart(chosen.tile, 0);
    } else if (candidates.length > 0) {
      // Past the last tile — pin to its end.
      const last = candidates[candidates.length - 1];
      if (last.text && last.text.nodeType === 3) {
        range.setStart(last.text, last.text.nodeValue.length);
      } else {
        range.setStart(last.tile, 0);
      }
    } else {
      range.selectNodeContents(el);
      range.collapse(false);
    }
    range.collapse(true);
    sel.removeAllRanges();
    sel.addRange(range);
  }, docOffset);
}

/**
 * Wait until the debug panel's reported doc length equals `n`.
 * Used to synchronize against the async DOM → state → render
 * loop after typing.
 */
async function waitForLen(page, n) {
  await expect(page.locator("#dbg-len")).toHaveText(String(n));
}

test.describe("editor", () => {
  test.beforeEach(async ({ page }) => {
    // Quieter test output — fail the test on console.error.
    page.on("pageerror", (err) => {
      throw err;
    });
    await page.goto("/?novim=1");
    // Wait for the editor to mount and the initial state to
    // populate the debug panel.
    await editor(page).waitFor();
    await expect(page.locator("#dbg-len")).not.toHaveText("");
    // `dx serve` injects a "Your app is being rebuilt." overlay
    // when it detects a non-hot-reloadable change. That overlay
    // sits above the editor and steals focus/clicks, looking
    // like a focus / state bug when it's really a hot-reload
    // race. Forcefully remove it (we don't need the dev affordance
    // during tests).
    await page.evaluate(() => {
      const heads = Array.from(document.querySelectorAll("h3"));
      for (const h of heads) {
        if (/being rebuilt/i.test(h.textContent || "")) {
          // Walk up to the overlay container (cursor=pointer
          // wrapper several levels up) and remove it.
          let n = h;
          while (n && n.parentElement && n.tagName !== "BODY") {
            const style = window.getComputedStyle(n);
            if (style.position === "fixed" || style.position === "absolute") {
              n.remove();
              return;
            }
            n = n.parentElement;
          }
          h.remove();
        }
      }
    });
  });

  test("renders the seeded document", async ({ page }) => {
    const state = await readState(page);
    // Seed text from main.rs — we don't assert exact bytes
    // (the welcome message may evolve), just that it's nonzero
    // and contains some recognizable substring.
    expect(Number(state.len)).toBeGreaterThan(0);
    expect(state.text).toContain("Editor");
    expect(state.text.toLowerCase()).toContain("preview");
  });

  test("types a character and the state grows by one", async ({ page }) => {
    const before = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, before); // caret at end
    // `insertText` fires the same DOM InputEvent the browser
    // emits for normal typing, which our MutationObserver
    // listens for. `keyboard.type` simulates key-down/up at the
    // OS level and on Linux/headless can miss the input event
    // entirely on contenteditable elements.
    await page.keyboard.insertText("x");
    await waitForLen(page, before + 1);
    const after = await readState(page);
    expect(Number(after.len)).toBe(before + 1);
    expect(after.text.endsWith("x")).toBeTruthy();
  });

  test("backspace via command removes the previous char", async ({ page }) => {
    const before = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, before);
    await page.keyboard.press("Backspace");
    await waitForLen(page, before - 1);
    expect(Number((await readState(page)).len)).toBe(before - 1);
  });

  test("Mod-A selects the visible document", async ({ page }) => {
    // Note: when the doc has Hidden tiles (markdown markers
    // hidden by the live-preview decoration), DOM Selection
    // can only span visible characters. CM6 keeps state's
    // selection authoritative even when DOM clamps, but our
    // bridge currently lets the clamp leak back into state
    // via the post-Mod-A keyup → sendSel path. So we verify
    // the visible portion got selected — the "user-perceived
    // select all" — and defer the Hidden-aware version.
    // FUTURE: Rust-side `pending writeback` flag in
    // push_selection.
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await page.keyboard.press("ControlOrMeta+a");
    await expect
      .poll(async () =>
        Math.abs(
          Number(await page.locator("#dbg-head").textContent()) -
            Number(await page.locator("#dbg-anchor").textContent())
        )
      )
      .toBeGreaterThan(0);
    const state = await readState(page);
    const anchor = Number(state.anchor);
    const head = Number(state.head);
    expect(Math.min(anchor, head)).toBe(0);
    // At minimum: selection should be longer than a single
    // char. The exact endpoint depends on how much of the doc
    // is currently visible.
    expect(Math.max(anchor, head)).toBeGreaterThan(20);
    expect(Math.max(anchor, head)).toBeLessThanOrEqual(len);
  });

  test("arrow key updates the caret position", async ({ page }) => {
    // The default seed opens with a ~329-byte YAML frontmatter block
    // that renders as the properties widget, so doc offset 5 falls
    // inside that replaced/hidden region with no text node to anchor a
    // caret — `setCaret(5)` can't land there. Use a plain-text seed
    // (same pattern as the other caret tests) so offset 5 is real,
    // editable text.
    await page.goto("/?seed=hello%20world%20text&novim=1");
    await editor(page).waitFor();
    await expect(page.locator("#dbg-len")).not.toHaveText("");
    await editor(page).focus();
    await setCaret(page, 5);
    // Give Phase-9 MutationObserver / selection bridge a tick
    // to sync state with the manual selection.
    await page.waitForFunction(() => {
      const el = document.querySelector("#dbg-head");
      return el && Number(el.textContent) === 5;
    });
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("#dbg-head")).toHaveText("4");
  });

  test("markdown bold round-trips through the visible-text mirror", async ({
    page,
  }) => {
    // Hidden markdown markers ( `**…**` etc. ) shrink the rendered
    // visible text, but typing should not drop the underlying
    // bytes from state.doc. Use a clean seed so we control the
    // exact bytes we're checking for.
    await page.goto("/?seed=hello%20**world**%20after&novim=1");
    await editor(page).waitFor();
    await expect.poll(async () => (await readState(page)).len).toBe("21");
    await editor(page).focus();
    await setCaret(page, 100_000);
    await page.keyboard.insertText("x");
    await waitForLen(page, 22);
    const after = await readState(page);
    expect(Number(after.len)).toBe(22);
    expect(after.text).toContain("**world**");
    expect(after.text.endsWith("x")).toBeTruthy();
  });

  test("multi-line: pressing Enter creates a new line tile", async ({
    page,
  }) => {
    // After beforeinput interception lands, Enter is authored
    // by Rust as a Change inserting "\n" at the caret, which
    // the tile-tree renderer turns into a new `.cm-line` div.
    // The browser never gets to insert its own <br> or <div>.
    const linesBefore = await page.locator(".cm-line").count();
    await editor(page).focus();
    await setCaret(page, 100_000); // end
    await page.keyboard.press("Enter");
    await page.keyboard.insertText("second line");
    // Wait for the .cm-line count to grow.
    await expect
      .poll(async () => page.locator(".cm-line").count())
      .toBeGreaterThan(linesBefore);
    const linesAfter = await page.locator(".cm-line").count();
    expect(linesAfter).toBeGreaterThan(linesBefore);
  });

  test("shift+arrow-left extends selection backward", async ({ page }) => {
    // Regression: backward-direction selections were collapsing
    // because placeSelection used `Range`/`addRange` which always
    // normalize start<=end. The browser would then think the
    // focus was on the right and shift+arrow-left would just
    // shrink the right edge instead of extending the left.
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("abcdef");
    await expect.poll(async () => (await readState(page)).len).toBe("6");
    // Move to position 5 with ArrowLeft (works against current
    // DOM state — setCaret races the patcher on a fresh insert).
    await page.keyboard.press("End");
    await page.keyboard.press("ArrowLeft");
    await expect.poll(async () => (await readState(page)).head).toBe("5");
    await page.keyboard.press("Shift+ArrowLeft");
    await page.keyboard.press("Shift+ArrowLeft");
    await page.keyboard.press("Shift+ArrowLeft");
    await expect.poll(async () => (await readState(page)).head).toBe("2");
    const s = await readState(page);
    expect(Number(s.anchor)).toBe(5);
    expect(Number(s.head)).toBe(2);
  });

  test("Enter at end of line with multi-byte chars splits AFTER the text", async ({
    page,
  }) => {
    // User-reported regression: hitting Enter at the end of a
    // line that contains an em-dash (3 UTF-8 bytes / 1 UTF-16
    // code unit) stranded the trailing characters on the new
    // line. Root cause: doc positions are UTF-8 byte offsets
    // but JS string offsets are UTF-16 code units; the cursor
    // walked 2 bytes short per em-dash.
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("ab — cd");
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    await page.keyboard.insertText("X");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("ab — cd\nX");
  });

  test("Bracket auto-closes and caret lands between", async ({ page }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("(");
    await expect.poll(async () => (await readState(page)).text).toBe("()");
    await expect.poll(async () => (await readState(page)).head).toBe("1");
  });

  test("Typing close bracket adjacent skips over", async ({ page }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("(");
    await expect.poll(async () => (await readState(page)).text).toBe("()");
    // Caret should be at 1 between `()`. Typing `)` should skip.
    await page.keyboard.insertText(")");
    await expect.poll(async () => (await readState(page)).text).toBe("()");
    await expect.poll(async () => (await readState(page)).head).toBe("2");
  });

  test("Backspace between empty pair deletes both", async ({ page }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("(");
    await page.keyboard.press("Backspace");
    await expect.poll(async () => (await readState(page)).text).toBe("");
  });

  test("Clicking a task checkbox toggles its source byte", async ({ page }) => {
    await page.goto("/?seed=&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    // Two lines so we can move the caret away from the task —
    // the checkbox widget only renders when the caret is OFF
    // the task line.
    await page.keyboard.insertText("- [ ] todo\nelsewhere");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- [ ] todo\nelsewhere");
    await page.evaluate(() => {
      document
        .querySelector(".md-task-checkbox")
        .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- [x] todo\nelsewhere");
  });

  test("Enter on a list item continues the list and cursor lands after marker", async ({
    page,
  }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    // Type the first item then Enter then type next.
    await page.keyboard.insertText("- foo");
    await expect.poll(async () => (await readState(page)).text).toBe("- foo");
    await page.keyboard.press("Enter");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- foo\n- ");
    await page.keyboard.insertText("bar");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- foo\n- bar");
  });

  test("Enter on a numbered list continues and increments", async ({ page }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("1. foo");
    await expect.poll(async () => (await readState(page)).text).toBe("1. foo");
    await page.keyboard.press("Enter");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("1. foo\n2. ");
    await page.keyboard.insertText("bar");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("1. foo\n2. bar");
  });

  test("Enter on a task item continues with unchecked box", async ({ page }) => {
    await page.goto("/?seed=&novim=1&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 0);
    await page.keyboard.insertText("- [x] done");
    await expect.poll(async () => (await readState(page)).text).toBe("- [x] done");
    await page.keyboard.press("Enter");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- [x] done\n- [ ] ");
    await page.keyboard.insertText("next");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("- [x] done\n- [ ] next");
  });

  test("Enter at end of decorated line splits AFTER the visible text", async ({
    page,
  }) => {
    // User-reported regression: clicking at the end of a line
    // that contains a `**bold**` span (markers hidden) and
    // pressing Enter moved trailing chars to the new line.
    // Root cause: browsers place the cursor at element-level
    // (cm-line div's child-index offset) when the click lands
    // past the last text node — selOffsets was reading that
    // offset as a character offset, which pointed to a position
    // inside the doc instead of at its visible end.
    await page.goto("/?seed=Hello%20**bold**%20world.&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    // Click at the visible end-of-line: place cursor at element
    // level via a click in the line div past its last text.
    const lineHandle = page.locator(".cm-line").first();
    const box = await lineHandle.boundingBox();
    await page.mouse.click(box.x + box.width - 2, box.y + box.height / 2);
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    await page.keyboard.insertText("X");
    await expect
      .poll(async () => (await readState(page)).text)
      .toBe("Hello **bold** world.\nX");
  });

  test("markdown bold via literal ** markers", async ({ page }) => {
    // Variant 1: user types the markdown markers manually,
    // same way you would in any plain markdown editor. The
    // doc text should contain the literal `**...**` exactly
    // as typed — markers stored, just hidden visually when
    // the caret leaves the span.
    await editor(page).focus();
    await setCaret(page, 100_000); // append at end
    // Insert a newline first so the test's content is on its
    // own line (cleaner doc assertions).
    await page.keyboard.press("Enter");
    const sample = "Testing **This is bold** This Isn't Bold";
    await page.keyboard.insertText(sample);
    // Wait for state to catch up — last batch of chars goes
    // through the visible→doc diff path.
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain(sample);
  });

  test.describe("toggle_bold (command logic)", () => {
    test.beforeEach(async ({ page }) => {
      // Catch page errors / browser-side panics in this
      // describe block too (the outer beforeEach's listener
      // doesn't survive the page.goto inside this beforeEach).
      page.on("pageerror", (err) => {
        // Log to stdout AND attach for the trace viewer.
        console.error("PAGEERROR:", err.message);
        console.error(err.stack);
        throw err;
      });
      page.on("console", (msg) => {
        if (msg.type() === "error") {
          console.error("CONSOLE ERROR:", msg.text());
        }
      });
      await page.goto("/?seed=&novim=1&novim=1");
      await editor(page).waitFor();
    });

    test("Mod-B inserts a bold pair at caret", async ({ page }) => {
      await editor(page).focus();
      await setCaret(page, 0);
      await page.keyboard.press("ControlOrMeta+b");
      await expect.poll(async () => (await readState(page)).text).toBe("****");
      // Caret parked between the two pairs.
      await expect.poll(async () => (await readState(page)).head).toBe("2");
    });

    test("Mod-B skips past closing marker", async ({ page }) => {
      // Seed the doc by typing `**hi**` and then position
      // the caret right before the closing `**`.
      await editor(page).focus();
      await setCaret(page, 0);
      await page.keyboard.insertText("**hi**");
      // Wait for BOTH state AND DOM to catch up before
      // setCaret, otherwise setCaret walks a DOM that hasn't
      // been rendered with the new tile tree yet.
      await expect.poll(async () => (await readState(page)).text).toBe("**hi**");
      await expect
        .poll(async () => await page.locator(".cm-line").first().textContent())
        .toBe("**hi**");
      await setCaret(page, 4);
      await expect
        .poll(async () => (await readState(page)).head)
        .toBe("4");
      await page.keyboard.press("ControlOrMeta+b");
      // Length unchanged, caret moved by 2.
      await expect.poll(async () => (await readState(page)).len).toBe("6");
      await expect.poll(async () => (await readState(page)).head).toBe("6");
    });

    // Two integration tests left red, intentionally. Both
    // exercise the same deeper bug: typing INSIDE an empty
    // Mark span hangs the page.
    //
    // What we have done so far:
    //   - render_dx emits the empty Mark as just `<span></span>`
    //     (no <br>) so the DOM shape stays consistent.
    //   - writeback's placeEdge has a second pass that
    //     specifically targets empty tiles by their `data-tile-pos`
    //     and parks the cursor INSIDE the empty Mark span.
    //   - JS bridge attempts to detect "cursor in empty mark"
    //     during beforeinput and route through our Rust-side
    //     before-input-insert handler.
    //
    // What's STILL broken:
    //   - Either the detection doesn't fire, or it does and
    //     the resulting state→render cascade still produces
    //     a structural DOM transition that hangs Dioxus's
    //     reconciler. Bisecting further requires either
    //     hooking into Dioxus's render lifecycle or
    //     replacing the contenteditable content with
    //     imperative DOM patching from Rust.
    //
    // For users today: select text BEFORE Mod-B to wrap a
    // selection — that path works perfectly.
    test("Mod-B sequence: typing inside empty span", async ({ page }) => {
      // Mod-B at start of empty doc → `****` with caret between.
      // Typing then has to drop characters INSIDE the empty Mark
      // span. The empty→non-empty Mark transition used to hang
      // the page under Dioxus's VDOM reconciler; with imperative
      // patching it should work.
      await editor(page).focus();
      await setCaret(page, 0);
      await page.keyboard.press("ControlOrMeta+b");
      await expect.poll(async () => (await readState(page)).text).toBe("****");
      await page.keyboard.insertText("X");
      await expect.poll(async () => (await readState(page)).text).toBe("**X**");
    });

    test("Mod-B sequence builds Testing **Bold** suffix", async ({ page }) => {
      const syncDom = async (expected) => {
        await expect.poll(async () => (await readState(page)).text).toBe(expected);
        await expect
          .poll(async () => {
            const lines = page.locator(".cm-line");
            const n = await lines.count();
            let acc = "";
            for (let i = 0; i < n; i++) {
              if (i > 0) acc += "\n";
              acc += (await lines.nth(i).textContent()) || "";
            }
            return acc;
          })
          .toBe(expected);
      };
      await editor(page).focus();
      await setCaret(page, 0);
      await page.keyboard.insertText("Testing ");
      await syncDom("Testing ");
      await page.keyboard.press("ControlOrMeta+b");
      await syncDom("Testing ****");
      await page.keyboard.insertText("This Is Bold");
      await syncDom("Testing **This Is Bold**");
      await page.keyboard.press("ControlOrMeta+b");
      await expect.poll(async () => (await readState(page)).head).toBe("24");
      await page.keyboard.insertText(" This Isn't Bold");
      await expect
        .poll(async () => (await readState(page)).text)
        .toBe("Testing **This Is Bold** This Isn't Bold");
    });

    test("literal **bold** typing produces the same final text", async ({
      page,
    }) => {
      // Variant 1: user types the markers manually.
      await editor(page).focus();
      await setCaret(page, 0);
      const sample = "Testing **This is bold** This Isn't Bold";
      await page.keyboard.insertText(sample);
      await expect.poll(async () => (await readState(page)).text).toBe(sample);
    });
  });

  test.describe("caret + selection across tiles", () => {
    test.beforeEach(async ({ page }) => {
      // Plain seed with multiple lines and a marked span so
      // we exercise the tile-tree position math. `nodeco=1`
      // keeps the live-preview decoration off — the click
      // tests target the *base* DOM↔doc translation, not the
      // decoration churn.
      await page.goto(
        "/?seed=Line%20one%0ALine%20two%0ALine%20three&nodeco=1&novim=1"
      );
      await editor(page).waitFor();
      await expect.poll(async () => (await readState(page)).len).toBe("28");
    });

    test("clicking inside a line positions caret there", async ({ page }) => {
      // Find the second cm-line, click at its first character.
      // The caret should land at doc position 9 ("Line one\n"
      // is 9 chars, second line starts at 9).
      const lines = page.locator(".cm-line");
      await expect(lines.nth(1)).toBeVisible();
      // Click at the START of line 2's text node.
      await lines.nth(1).click({ position: { x: 1, y: 5 } });
      await expect
        .poll(async () => Number((await readState(page)).head))
        .toBeGreaterThanOrEqual(9);
      await expect
        .poll(async () => Number((await readState(page)).head))
        .toBeLessThan(13); // somewhere on line 2
    });

    test("shift+arrow extends selection across lines", async ({ page }) => {
      // Caret at start of line 2 (doc 9). Shift+ArrowDown
      // should extend the selection to line 3's same column,
      // crossing the BreakAfter on line 2.
      await editor(page).focus();
      await setCaret(page, 9);
      await expect.poll(async () => (await readState(page)).head).toBe("9");
      await page.keyboard.press("Shift+ArrowDown");
      // After shift-down, head moves to next line. Anchor stays
      // at 9. The exact head depends on browser's column
      // tracking, but it should be > 9 and < doc.len.
      const after = await readState(page);
      expect(Number(after.anchor)).toBe(9);
      expect(Number(after.head)).toBeGreaterThan(9);
      expect(Number(after.head)).toBeLessThanOrEqual(Number(after.len));
    });
  });

  test.describe("cursor preservation", () => {
    // Regression coverage for the "every render resets the
    // cursor to position 0" bug. The bug was: the state→DOM
    // selection writeback compared `el.textContent` against
    // `state.doc`, but textContent doesn't include `\n` between
    // block children — so multi-line docs always failed the
    // check, the writeback never ran, and the cursor sat
    // wherever Dioxus's reconciler had dropped it after each
    // mutation (often offset 0).
    //
    // Each test below pins down a different way the bug
    // surfaced. Add new tests here when you find another.
    test.beforeEach(async ({ page }) => {
      // Use `nodeco=1` for the basic cases — the bug exists
      // independent of decorations. Decoration-specific
      // variants live in the separate block below.
      await page.goto(
        "/?seed=Line%20one%0ALine%20two%0ALine%20three&nodeco=1&novim=1"
      );
      await editor(page).waitFor();
      await expect.poll(async () => (await readState(page)).len).toBe("28");
    });

    test("cursor stays at end-of-doc after typing a character", async ({
      page,
    }) => {
      await editor(page).focus();
      // Place at end of doc (pos 28).
      await setCaret(page, 28);
      await expect.poll(async () => (await readState(page)).head).toBe("28");
      await page.keyboard.insertText("X");
      // Cursor should be at 29, NOT 0.
      await expect.poll(async () => (await readState(page)).head).toBe("29");
      await expect.poll(async () => (await readState(page)).text).toBe(
        "Line one\nLine two\nLine threeX"
      );
    });

    test("cursor stays mid-line after typing", async ({ page }) => {
      // Caret in the middle of line 2 (between "Line " and
      // "two"). doc pos = 9 + 5 = 14.
      await editor(page).focus();
      await setCaret(page, 14);
      await expect.poll(async () => (await readState(page)).head).toBe("14");
      await page.keyboard.insertText("X");
      // Cursor advances to 15 — NOT 0, NOT end-of-doc.
      await expect.poll(async () => (await readState(page)).head).toBe("15");
    });

    test("cursor stays through multiple keystrokes in middle of doc", async ({
      page,
    }) => {
      // Type a multi-char burst in the middle and verify the
      // cursor advances one char per char, never resetting.
      await editor(page).focus();
      await setCaret(page, 14);
      const before = "abcde";
      for (let i = 0; i < before.length; i++) {
        await page.keyboard.insertText(before[i]);
        // After each char, cursor must be at the previous
        // position + 1 — never 0, never anywhere else.
        const expected = String(14 + i + 1);
        await expect
          .poll(async () => (await readState(page)).head)
          .toBe(expected);
      }
    });

    test("cursor stays after backspace mid-line", async ({ page }) => {
      await editor(page).focus();
      await setCaret(page, 14);
      await expect.poll(async () => (await readState(page)).head).toBe("14");
      await page.keyboard.press("Backspace");
      // Cursor moves to 13, NOT to 0.
      await expect.poll(async () => (await readState(page)).head).toBe("13");
    });

    test("cursor stays after pressing Enter mid-line", async ({ page }) => {
      // Enter is routed through beforeinput-insert which splits
      // a line — exactly the kind of structural mutation that
      // used to orphan the cursor.
      await editor(page).focus();
      await setCaret(page, 14);
      await page.keyboard.press("Enter");
      // Cursor moves +1 (past the inserted \n), NOT 0.
      await expect.poll(async () => (await readState(page)).head).toBe("15");
      // And there's an extra line now.
      const after = await readState(page);
      expect(Number(after.len)).toBe(29);
    });

    test("cursor across many small edits never collapses to 0", async ({
      page,
    }) => {
      // The deepest version of the regression: a long sequence
      // of typing + backspace + arrow operations. After each
      // step the cursor must be at a sensible non-zero
      // position. We log every cursor jump so any future
      // regression points at the specific operation.
      await editor(page).focus();
      await setCaret(page, 9); // start of line 2
      await expect.poll(async () => (await readState(page)).head).toBe("9");
      for (const ch of "Hello") {
        const before = Number((await readState(page)).head);
        await page.keyboard.insertText(ch);
        const after = Number((await readState(page)).head);
        expect(after, `after typing '${ch}' from ${before}`).toBe(before + 1);
      }
      // ArrowLeft x3: each step head -= 1, never collapses.
      for (let i = 0; i < 3; i++) {
        const before = Number((await readState(page)).head);
        await page.keyboard.press("ArrowLeft");
        const after = Number((await readState(page)).head);
        expect(after, `after ArrowLeft from ${before}`).toBe(before - 1);
      }
      // Backspace twice: head -= 1, len -= 1, never collapses.
      for (let i = 0; i < 2; i++) {
        const before = Number((await readState(page)).head);
        const lenBefore = Number((await readState(page)).len);
        await page.keyboard.press("Backspace");
        const after = await readState(page);
        expect(Number(after.head), `head after BS from ${before}`).toBe(
          before - 1
        );
        expect(Number(after.len), `len after BS from ${lenBefore}`).toBe(
          lenBefore - 1
        );
      }
    });

    test("cursor never resets to 0 across many random renders", async ({
      page,
    }) => {
      // Stress-style. Type a bunch of chars at the end of the
      // doc. Each render emits a new tile arena (fresh
      // data-tile-ids), which used to be what triggered the
      // bug. Asserts the cursor matches what it should be at
      // every step.
      await editor(page).focus();
      await setCaret(page, 28);
      const expectedFinal = 28 + 30;
      for (let i = 0; i < 30; i++) {
        await page.keyboard.insertText("z");
        const got = Number((await readState(page)).head);
        // The cursor must NEVER be 0 once we've started typing.
        expect(got, `iteration ${i}`).toBeGreaterThan(0);
      }
      // Final position lands at end of inserts.
      await expect
        .poll(async () => Number((await readState(page)).head))
        .toBe(expectedFinal);
    });
  });

  test.describe("cursor preservation with live-preview decorations", () => {
    // Same regressions as the block above, but with the
    // markdown decoration source ON. Decoration shape changes
    // mid-typing (e.g., cursor leaving a `**...**` span makes
    // the markers hide → tile-tree rebuilds → text nodes get
    // replaced) are the most aggressive form of the "cursor
    // gets orphaned" bug. These tests pin it down.
    test.beforeEach(async ({ page }) => {
      // Seed: "hello **world** after" — 21 doc bytes. When
      // caret sits outside the **world** span, live_preview
      // hides the markers (Hidden tiles), so the rendered
      // visible text is "hello world after" (17 chars).
      await page.goto("/?seed=hello%20**world**%20after&novim=1");
      await editor(page).waitFor();
      await expect.poll(async () => (await readState(page)).len).toBe("21");
    });

    test("typing after a decorated span keeps cursor at end", async ({
      page,
    }) => {
      await editor(page).focus();
      // Place caret at end of doc (pos 21).
      await setCaret(page, 21);
      await expect.poll(async () => (await readState(page)).head).toBe("21");
      await page.keyboard.insertText("!");
      // Cursor at 22, NOT 0.
      await expect.poll(async () => (await readState(page)).head).toBe("22");
    });

    test("cursor advances each step when typing many chars at end", async ({
      page,
    }) => {
      await editor(page).focus();
      await setCaret(page, 21);
      for (let i = 0; i < 10; i++) {
        const before = Number((await readState(page)).head);
        await page.keyboard.insertText("x");
        const after = Number((await readState(page)).head);
        expect(after, `step ${i}: head went ${before} → ${after}`).toBe(
          before + 1
        );
      }
    });

    test("typing past a closing marker doesn't garble the text", async ({
      page,
    }) => {
      // User-reported regression: typing
      //   "Something **Testing** Landed Here"
      // produced
      //   "Somlandeedherething **Testing**"
      // because the caret transitioned from inside the bold span
      // (markers visible, one big text node) to outside (markers
      // hidden, multiple tiles). The DOM restructured, the cursor
      // was orphaned, and the next chars landed at offset 0.
      await page.goto("/?seed=&novim=1&novim=1");
      await editor(page).waitFor();
      await editor(page).focus();
      await setCaret(page, 0);
      await page.keyboard.insertText("Something ");
      await page.keyboard.insertText("**Testing** ");
      await page.keyboard.insertText("Landed Here");
      await expect
        .poll(async () => (await readState(page)).text)
        .toBe("Something **Testing** Landed Here");
    });

    test("cursor doesn't collapse when caret crosses decoration boundary", async ({
      page,
    }) => {
      // Place caret inside the **world** span at doc 9 (the
      // 'w'). live_preview keeps markers visible while caret
      // touches the span. Move caret right past the closing
      // `**` (doc 15) — markers should now hide, which
      // rebuilds the tile tree. The crucial property is
      // "cursor does NOT collapse to 0" — what the
      // user-reported bug looked like.
      await editor(page).focus();
      await setCaret(page, 9);
      await expect.poll(async () => (await readState(page)).head).toBe("9");
      for (let i = 0; i < 8; i++) {
        await page.keyboard.press("ArrowRight");
      }
      const head = Number((await readState(page)).head);
      expect(head, "cursor did NOT collapse to 0").toBeGreaterThan(0);
      expect(head).toBeGreaterThan(9);
    });
  });

  test("caret can land at pos 0 on an ordered list line", async ({ page }) => {
    // Regression: list-marker Replace decorations used to swallow
    // the marker bytes, so the cursor couldn't land at byte 0 of
    // a list line — placeSelection fell through to the line-tile
    // end fallback. After the fix, the marker source stays visible
    // when the caret is on the line, so placing the caret at
    // pos 0 actually sticks instead of bouncing to end-of-line.
    await page.goto("/?seed=1.%20foo&novim=1");
    await editor(page).waitFor();
    await expect.poll(async () => (await readState(page)).text).toBe("1. foo");
    await editor(page).focus();
    await setCaret(page, 0);
    // Give the bridge a tick to sync the DOM selection back to state.
    await expect.poll(async () => (await readState(page)).head).toBe("0");
  });

  test("vim b on a list line walks left without bouncing to EOL", async ({
    page,
  }) => {
    // With vim on, repeated `b` from the end of `1. foo` must
    // eventually land at byte 0 and stop. Before the fix, the
    // list-marker Replace bytes were unreachable so placeSelection
    // bounced the caret to end-of-line on every `b` keypress.
    await page.goto("/?seed=1.%20foo");
    await editor(page).waitFor();
    await editor(page).focus();
    await expect.poll(async () => (await readState(page)).text).toBe("1. foo");
    await setCaret(page, 6); // end of line
    await expect.poll(async () => (await readState(page)).head).toBe("6");
    // `b` repeatedly — at most a handful of steps to land at 0.
    let last = 6;
    for (let i = 0; i < 6; i++) {
      await page.keyboard.press("b");
      const head = Number((await readState(page)).head);
      // Never bounces to EOL after the first step.
      expect(head, `iter ${i} (prev ${last})`).toBeLessThanOrEqual(last);
      last = head;
      if (head === 0) break;
    }
    expect(last).toBe(0);
    // One more `b` at pos 0 must stay at 0 — no infinite bounce.
    await page.keyboard.press("b");
    await expect.poll(async () => (await readState(page)).head).toBe("0");
  });

  test("typing does not lose characters under fast input", async ({
    page,
  }) => {
    const before = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, before);
    const burst = "abcdefghijklmnopqrst";
    await page.keyboard.insertText(burst);
    await waitForLen(page, before + burst.length);
    const after = await readState(page);
    expect(after.text.endsWith(burst)).toBeTruthy();
  });

  // ── Frontmatter properties ───────────────────────────────
  //
  // The seeded doc starts with a `---\n…\n---\n` block. The
  // editor renders it as a `.md-properties` widget; the YAML
  // bytes are still in the doc, just hidden behind a Replace
  // decoration.

  test("frontmatter renders as a properties widget", async ({ page }) => {
    await expect(page.locator(".md-properties")).toBeVisible();
    // Each prop produces one row.
    const rows = page.locator(".md-property-row");
    await expect(rows).not.toHaveCount(0);
    // The `tags` key from the seed should be present.
    await expect(
      page.locator('.md-property-row[data-prop-key="tags"]')
    ).toBeVisible();
  });

  test("toggling a bool property flips the source", async ({ page }) => {
    const beforeText = (await readState(page)).text;
    // `published: true` is in the seed — the cell carries
    // `data-edit-role="bool"` with `data-checked="true"`.
    const cell = page
      .locator('.md-property-row[data-prop-key="published"] [data-edit-role="bool"]')
      .first();
    await cell.click();
    await expect
      .poll(async () => (await readState(page)).text)
      .not.toBe(beforeText);
    const after = (await readState(page)).text;
    expect(after).toContain("published: false");
  });

  /**
   * Focus a property cell and replace its text content with
   * `value`, then blur it. Drives the cell directly via DOM
   * APIs instead of relying on keyboard.press("ControlOrMeta+a"),
   * which races Dioxus's document-level event delegation and
   * ends up selecting-all on the whole editor doc.
   */
  async function setPropertyCellText(page, key, role, value) {
    await page.evaluate(
      ({ key, role, value }) => {
        const sel = `.md-property-row[data-prop-key="${key}"] [data-edit-role="${role}"]`;
        const cell = document.querySelector(sel);
        if (!cell) throw new Error(`cell ${sel} not found`);
        cell.focus();
        // Replace the cell's text content, then fire blur so
        // the editor's focusout handler dispatches prop-set.
        cell.textContent = value;
        cell.blur();
      },
      { key, role, value }
    );
  }

  test("editing a text property writes back through YAML", async ({
    page,
  }) => {
    // Sanity: cell exists and shows the seed value.
    const cell = page.locator(
      '.md-property-row[data-prop-key="title"] [data-edit-role="text"]'
    );
    await expect(cell).toHaveText("Editor playground");
    await setPropertyCellText(page, "title", "text", "Renamed Title");
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain("title: Renamed Title");
  });

  test("editing a number property writes back through YAML", async ({
    page,
  }) => {
    await setPropertyCellText(page, "priority", "number", "7");
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain("priority: 7");
  });

  test("editing a date property writes back through YAML", async ({
    page,
  }) => {
    // Date cell is a native `<input type="date">`. Set its
    // `value` then fire a `change` event so the editor's
    // change-handler dispatches prop-set.
    await page.evaluate(() => {
      const input = document.querySelector(
        '.md-property-row[data-prop-key="created"] [data-edit-role="date"]'
      );
      input.value = "2030-01-15";
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain("created: 2030-01-15");
  });

  test("adding a list chip appends to the YAML block list", async ({
    page,
  }) => {
    // Drive the chip-add cell directly: set its text, fire an
    // Enter keydown, then verify the new tag appears in the
    // YAML.
    await page.evaluate(() => {
      const add = document.querySelector(
        '.md-property-row[data-prop-key="tags"] [data-edit-role="chip-add"]'
      );
      add.focus();
      add.textContent = "new-tag";
      add.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
        })
      );
    });
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain("- new-tag");
  });

  test("removing a list chip drops it from the YAML block list", async ({
    page,
  }) => {
    // Click the `×` button on the first existing chip.
    await page.evaluate(() => {
      const x = document.querySelector(
        '.md-property-row[data-prop-key="tags"] .md-property-chip [data-edit-role="chip-remove"]'
      );
      x.click();
    });
    // The chip that's removed is the first one in the seed
    // (`editor`). The remaining list still contains `demo` and
    // `live-preview`.
    await expect
      .poll(async () => (await readState(page)).text)
      .toMatch(/tags:\n  - demo\n  - live-preview\n/);
  });

  test("removing a property row deletes its YAML line", async ({ page }) => {
    // `author` is a single-line scalar — easy to verify it's
    // gone from the doc text.
    await page.evaluate(() => {
      const row = document.querySelector(
        '.md-property-row[data-prop-key="author"]'
      );
      const x = row.querySelector('[data-edit-role="row-remove"]');
      x.click();
    });
    await expect
      .poll(async () => (await readState(page)).text)
      .not.toContain("author: cody");
  });

  test("arrow up from a property cell focuses the previous cell", async ({
    page,
  }) => {
    // Drive the focus + keydown entirely via `page.evaluate` so
    // we sidestep Playwright's keyboard layer (which doesn't
    // consistently dispatch keydowns past Dioxus's document-
    // level delegation in this test rig).
    const after = await page.evaluate(() => {
      const add = document.querySelector('[data-edit-role="row-add"]');
      add.focus();
      add.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "ArrowUp",
          bubbles: true,
          cancelable: true,
        })
      );
      const a = document.activeElement;
      return {
        role: a && a.dataset && a.dataset.editRole,
        rowKey:
          a && a.closest && a.closest(".md-property-row")
            ? a.closest(".md-property-row").dataset.propKey
            : null,
      };
    });
    expect(after.role).toBeTruthy();
    expect(after.rowKey).toBe("description");
  });

  test("arrow down from the title cell focuses the tags row", async ({
    page,
  }) => {
    const after = await page.evaluate(() => {
      const cell = document.querySelector(
        '.md-property-row[data-prop-key="title"] [data-edit-role="text"]'
      );
      cell.focus();
      cell.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "ArrowDown",
          bubbles: true,
          cancelable: true,
        })
      );
      const a = document.activeElement;
      return {
        rowKey:
          a && a.closest && a.closest(".md-property-row")
            ? a.closest(".md-property-row").dataset.propKey
            : null,
      };
    });
    expect(after.rowKey).toBe("tags");
  });

  test("adding a new property inserts an empty entry", async ({ page }) => {
    // Type into the "+ Add property" cell, press Enter, expect
    // the doc to contain a new `keyname:` line at the bottom
    // of the YAML.
    await page.evaluate(() => {
      const add = document.querySelector(
        '[data-edit-role="row-add"]'
      );
      add.focus();
      add.textContent = "newkey";
      add.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
        })
      );
    });
    await expect
      .poll(async () => (await readState(page)).text)
      .toContain("newkey:");
  });

  // ── Slash command palette ──────────────────────────────────
  //
  // `?novim=1` is on the page so we're plain-insert-mode by
  // default. Each test sets caret to a known position then
  // types — the menu should open whenever `/` is at start of
  // line / after whitespace, and stay closed elsewhere.

  test("slash opens the menu at start of line", async ({ page }) => {
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len); // end of doc
    // Insert a newline first so we know `/` is at line start.
    await page.keyboard.insertText("\n/");
    await expect(page.locator(".slash-menu")).toBeVisible();
    // Catalog renders at least the headings group.
    await expect(page.locator(".slash-group", { hasText: "Heading" })).toBeVisible();
  });

  test("typing after slash filters the catalog", async ({ page }) => {
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    await page.keyboard.insertText("\n/callout");
    await expect(page.locator(".slash-menu")).toBeVisible();
    // After the label rename, callouts are matched by their
    // group header ("Callout"); the labels themselves are just
    // the type name (Note / Todo / ...).
    await expect(page.locator(".slash-group", { hasText: "Callout" })).toBeVisible();
    await expect(page.locator(".slash-row-label", { hasText: "Code block" })).toHaveCount(0);
  });

  test("escape closes the slash menu", async ({ page }) => {
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    await page.keyboard.insertText("\n/");
    await expect(page.locator(".slash-menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(".slash-menu")).toHaveCount(0);
  });

  test("enter picks the highlighted command", async ({ page }) => {
    const beforeLen = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, beforeLen);
    // Query has to be space-free — `detect_slash` closes the
    // menu when a whitespace char appears in the segment after
    // the trigger (matches Notion / Logseq). `/heading` is
    // unambiguous: it matches all six Heading rows; the first
    // (Heading 1) is selected by default.
    await page.keyboard.insertText("\n/heading");
    await expect(page.locator(".slash-menu")).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page.locator(".slash-menu")).toHaveCount(0);
    // After picking Heading 1, the line should end with `# `
    // (slash + query removed, heading prefix applied to the
    // now-empty line).
    await expect
      .poll(async () => (await readState(page)).text)
      .toMatch(/# $/);
  });

  test("slash inside a URL does NOT open the menu", async ({ page }) => {
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    // `://` URL pattern shouldn't trigger; the `/` after the
    // scheme colon is suppressed by detect_slash.
    await page.keyboard.insertText("\nhttps://");
    await expect(page.locator(".slash-menu")).toHaveCount(0);
  });

  test("slash after ordinary text opens the menu", async ({ page }) => {
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    // Typing `/` at the end of prose should open the menu
    // (Notion-style — the previous Logseq-strict rule was too
    // restrictive in a markdown editor where users type
    // continuously).
    await page.keyboard.insertText("\nhello/");
    await expect(page.locator(".slash-menu")).toBeVisible();
  });

  test("slash after typing letters opens the menu (vim default-on)", async ({
    page,
  }) => {
    // Type some letters first, then `/` — the exact flow the
    // user reported broken. Letters end up as ordinary text on
    // the line; `/` after them should still trigger the menu
    // (Notion rule).
    await page.goto("/");
    await editor(page).waitFor();
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    await page.keyboard.press("i"); // → Insert
    await page.keyboard.press("Enter");
    await page.keyboard.press("h");
    await page.keyboard.press("e");
    await page.keyboard.press("l");
    await page.keyboard.press("l");
    await page.keyboard.press("o");
    await page.keyboard.press("Space");
    await page.keyboard.press("Slash");
    await expect(page.locator(".slash-menu")).toBeVisible();
  });

  test("slash via real keystrokes (vim default-on) opens the menu", async ({
    page,
  }) => {
    // Reload without ?novim=1 so the default vim mode is on.
    await page.goto("/");
    await editor(page).waitFor();
    const len = Number((await readState(page)).len);
    // Vim defaults to Normal. Move to end, enter insert, then
    // start a new line and press slash — same flow a real user
    // would do.
    await editor(page).focus();
    await setCaret(page, len);
    await page.keyboard.press("i");  // vim → Insert
    await page.keyboard.press("Enter");
    await page.keyboard.press("Slash");
    await expect(page.locator(".slash-menu")).toBeVisible();
  });

  test("slash via real keystrokes opens the menu", async ({ page }) => {
    // The other slash tests use `keyboard.insertText` which
    // synthesizes a single composite input event. The real
    // failure mode showed up only when typing character-by-
    // character via `keyboard.press`: each keystroke goes
    // through keydown → contenteditable → MutationObserver →
    // state.set → use_effect. If any link in that chain breaks,
    // the menu silently doesn't open.
    const len = Number((await readState(page)).len);
    await editor(page).focus();
    await setCaret(page, len);
    await page.keyboard.press("Enter");
    await page.keyboard.press("Slash");
    await expect(page.locator(".slash-menu")).toBeVisible();
  });

  // ── Wikilink rendering ─────────────────────────────────────

  test("wikilinks render with the unresolved class by default", async ({
    page,
  }) => {
    // The seed contains `[[Editor Roadmap]]` (real wikilink,
    // not in backticks). With no vault, every wikilink is
    // unresolved → `.md-wikilink-unresolved`.
    await expect(
      page.locator(".md-wikilink-unresolved").first()
    ).toBeVisible();
  });

  // ── Inline footnote ────────────────────────────────────────

  test("inline footnote renders as a marker when caret away", async ({
    page,
  }) => {
    // Marker should be visible since the seed's footnote sits
    // far from doc start (default caret is at offset 0).
    await expect(
      page.locator(".md-inline-footnote-marker").first()
    ).toBeVisible();
  });

  // ── Nested callouts ────────────────────────────────────────

  // ── Block IDs + refs ─────────────────────────────────────

  test("block id property line is hidden from the rendered view", async ({
    page,
  }) => {
    await page.goto(
      "/?seed=" +
        encodeURIComponent(
          "Block content here\nid:: 5f9c1234-abcd-4ef0-8123-fedcba012345"
        ) +
        "&novim=1"
    );
    await editor(page).waitFor();
    // The id:: line shouldn't appear in the rendered text.
    const visible = await page.evaluate(
      () => document.querySelector("[data-editor-id]").innerText
    );
    expect(visible).not.toContain("id::");
    expect(visible).not.toContain("5f9c1234");
    expect(visible).toContain("Block content here");
  });

  test("block reference renders as a chip showing target preview", async ({
    page,
  }) => {
    const uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
    await page.goto(
      "/?seed=" +
        encodeURIComponent(
          `First block content\nid:: ${uuid}\n\nSecond block: ((${uuid})).`
        ) +
        "&novim=1"
    );
    await editor(page).waitFor();
    await expect(page.locator(".md-block-ref-chip").first()).toBeVisible();
    // Chip text should preview the target block's content.
    const chipText = await page
      .locator(".md-block-ref-chip")
      .first()
      .textContent();
    expect(chipText).toContain("First block content");
  });

  test("Mod-Shift-K appends an id:: line below the current block", async ({
    page,
  }) => {
    await page.goto("/?seed=" + encodeURIComponent("Hello world.") + "&novim=1");
    await editor(page).waitFor();
    await editor(page).focus();
    await setCaret(page, 3);
    await page.keyboard.press("Control+Shift+k");
    await expect
      .poll(async () => (await readState(page)).text)
      .toMatch(/Hello world\.\nid:: [0-9a-f-]{36}/);
  });

  test("nested callouts emit a depth class", async ({ page }) => {
    // Seed has `> > [!warning]` and `> > > [!danger]` blocks;
    // the depth-2 and depth-3 line classes should be on
    // rendered lines.
    await expect(page.locator(".md-callout-nested-2").first()).toBeVisible();
    await expect(page.locator(".md-callout-nested-3").first()).toBeVisible();
  });
});
