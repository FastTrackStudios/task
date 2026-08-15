# Editor integration — record

Two commits:
- `5983d92` feat(editor): integrate into Task workspace,
  wire EditorApp into task-ui
- `eff74e3` nix(flake): add wasm C toolchain to default
  shell, fold editor's shell in

## Before

The editor crates lived at `external/editor/` as a git subtree
imported from upstream Editor. `task-ui` mounted the legacy
`editor_outliner::EditorApp` (Logseq-style outliner under
`crates/editor-outliner/`). The new CodeMirror-style editor
was only consumed by `editor-state` (one trait used by
`vault::lookup` for cross-page ref resolution) — its actual
`Editor` component was unwired.

## What shipped

### Editor moved into the workspace
- `git mv external/editor/ features/editor/`. Subtree
  relationship abandoned by user direction — no more
  `git subtree pull` from upstream. Workspace member list +
  path deps updated; intermediate `crates/` / `vendor/` /
  `examples/` / `docs/` / `plans/` / `tests/` dirs added to
  `workspace.exclude` so the `features/*/*` glob doesn't try
  to parse them as crates.

### Standalone-workspace cruft removed
Deleted: `features/editor/Cargo.toml` (inner workspace),
`flake.nix`, `flake.lock`, `rust-toolchain.toml`,
`LICENSE-MIT`, `README.md`, `justfile`. Moved `Dioxus.toml`
next to the playground package it configures.

### `editor::EditorApp` turnkey component
Added a Dioxus component that mounts the `Editor` widget with
the standard markdown setup (live-preview decorations,
bracket matching, vim modal editing, slash-command palette,
bundled stylesheet). `task-ui::App` now mounts it instead of
`editor_outliner::EditorApp`. The CSS is the playground's
`assets/playground.css` copied to
`features/editor/crates/editor/assets/editor.css`.

### Wasm dev-shell fix
The editor's `editor-syntax` → `arborium` → `arborium-sysroot`
chain entered the wasm closure for the first time. The nix
dev shell's wrapped clang injects
`-fzero-call-used-regs=used-gpr`, which wasm32 rejects.

Fix lifted verbatim from the editor's old standalone flake:
```nix
devShells.default = pkgs.mkShell {
  inputsFrom = [ inputs.dioxus-flake.devShells.${system}.default ];
  packages = with pkgs; [
    llvmPackages.clang-unwrapped
    llvmPackages.bintools-unwrapped
  ];
  shellHook = ''
    export CC_wasm32_unknown_unknown=${pkgs.llvmPackages.clang-unwrapped}/bin/clang
    export AR_wasm32_unknown_unknown=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-ar
  '';
};
```

`cc::Build` picks the unwrapped toolchain only for the wasm
target; native compiles still go through the wrapped gcc.

### Playwright shell preserved
`devShells.playwright` was already in `flake.nix`. Folded in
`pnpm` + `just` to match what the editor's old standalone
flake provided; tests at `features/editor/tests/` still run
via `nix develop .#playwright` → `cd features/editor/tests`
→ `pnpm test`. The `webServer` `cwd` in
`playwright.config.js` was retargeted from
`features/editor/` (the editor's old repo root) to the Task
workspace root.

## What's parked

`crates/legacy/editor-outliner/` — the older Logseq-style
outliner. Parked under crates/legacy when the project-CRDT
rip swept through. Kept for mining patterns when the
`/projects` route or similar Logseq-flavored views are
rebuilt.
