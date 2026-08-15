# Web load time

**Status:** brotli + cache shipped (2026-07-01); wasm-split parked
with findings.

## Shipped

- The wasm was 42.7MB served uncompressed — ~6s first load. The
  webapp bundle now pre-compresses (brotli q9) and the images serve
  `.br` variants (`--compression-static`) + cache-control headers:
  **13.5MB wire**, and content-hashed assets make repeat visits
  browser-cached (near-instant).

## Parked: `dx build --wasm-split`

Tried on dx 0.7.6 (2026-07-01). Findings:

- Splitting is driven by `#[wasm_split]` annotations — it is NOT
  automatic per-route. With zero annotations the output was a
  313-byte `chunk_0` plus a main bundle *bigger* than baseline
  (49.8MB — the split pipeline wants `--emit-relocs`, and the
  symbol pass logged `Could not find function symbol "abort" …
  was this built with LTO?`, so the workspace release profile's
  LTO likely needs to be off for the split build).

To make it pay:

1. Annotate lazy boundaries around the heavy leaf subsystems —
   editor, scripture interlinear (original-language editions),
   email client, gantt/calendar views — so the first paint ships
   the shell + task list only.
2. Check the release profile interaction (LTO off for web?) and
   re-measure; the split must beat 13.5MB-brotli-cached to matter.
3. Only then wire `--wasm-split` into the flake's dx invocation.

## Other levers not yet pulled

- brotli `--quality=11` (slower build, a bit smaller).
- `[web.wasm_opt] level = "z"` in `apps/web/Dioxus.toml`.
- Feature-gate rarely-used routes out of the web build entirely.
