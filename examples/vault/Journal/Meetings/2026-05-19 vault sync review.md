---
title: 2026-05-19 vault sync review
type: synthesis
tags: [meeting, vault-sync]
date: 2026-05-19
sources: ["plans/vault-sync-vox-migration.md"]
folder: "[[Meetings]]"
---

# 2026-05-19 vault sync review

## Decisions

- Migrate [[Vault sync]] from raw axum REST to `#[architect::rpc]`.
- Keep the byte-level surface; defer richer `VaultObsidian` trait.
- `vault::Backend` becomes the canonical impl; server drops its parallel state.

## Action items

- [x] Move `crates/vault-sync-proto` → `features/vault/vault-proto/`.
- [x] Wire watcher → broadcast.
- [ ] Auth middleware on the mount arm.

Related: [[Vault sync]], [[Architect]], [[2026-05-20 editor wire-in]].
