# Playwright tests

Browser tests for the Editor playground. Modeled on Dioxus's own
[`playwright-tests`](https://github.com/DioxusLabs/dioxus/tree/main/packages/playwright-tests)
setup.

## Running (quick)

From the **repo root**:

```sh
just test                        # headless, full suite
just test-headed                 # opens a real Chromium window
just test-ui                     # interactive Playwright UI
just test-only "cursor stays"    # filter by substring
```

`just test` handles `pnpm install` for you. Chromium and Node
come from the nix flake — no `pnpm install-browsers` step.

## Running (without `just`)

From this directory:

```sh
pnpm install               # one-time
pnpm test                  # headless
pnpm test:headed           # opens a real Chromium window
pnpm test:ui               # interactive UI mode
```

The `playwright.config.js` `webServer` entry boots
`dx serve --platform web --port 8090` automatically; you don't
need to start the playground separately. First run takes a
while because `dx` builds the wasm bundle.

## What's covered

See `editor.spec.js`. Each test name describes the behavior:

- Initial render of the seeded document
- Single-character typing
- Backspace via the keymap command
- `Mod-A` (select all)
- ArrowLeft caret motion + selection mirror
- Markdown bold round-trip through the live-preview decoration
- Multi-line Enter creating new `.cm-line` tiles
- Burst typing doesn't lose characters

The tests read live state from the playground's debug panel
(`#dbg-len`, `#dbg-anchor`, `#dbg-head`, `#dbg-text`) — those
ids are stable assertion points the playground emits.

## Adding tests

When you add a behavior to the editor, add a corresponding test
here. Use the existing helpers:

- `editor(page)` — locator for the contenteditable
- `setCaret(page, offset)` — place the caret at a visible-text
  offset (handles multi-tile DOM correctly)
- `readState(page)` — snapshot of the debug panel as
  `{ len, anchor, head, text }`
- `waitForLen(page, n)` — synchronize against the async
  DOM → state → render loop
