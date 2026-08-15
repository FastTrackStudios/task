---
title: 2026-05-20 editor wire-in
type: synthesis
tags: [meeting, editor]
date: 2026-05-20
sources: ["plans/loro-text-editor-upgrade.md"]
folder: "[[Meetings]]"
---

# 2026-05-20 editor wire-in

## Decisions

- Move `external/editor/` → `features/editor/` (fully Task-owned).
- Add `editor::EditorApp` turnkey component.
- Wire `task-ui::App` to use it; legacy `editor-outliner` parked.

## Action items

- [x] Move + integrate.
- [x] Bundle CSS.
- [x] Fix wasm dev shell (`clang-unwrapped`, `CC_wasm32_unknown_unknown`).
- [ ] Vault-aware decorations in `EditorApp` (so cross-doc refs resolve).

Related: [[Editor]], [[Live preview]], [[2026-05-19 vault sync review]].
