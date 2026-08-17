---
title: Vault sync
type: synthesis
tags: [project, sync, crdt]
sources: ["plans/vault-sync-vox-migration.md"]
status: shipped
priority: 1
folder: "[[Projects]]"
---

# Vault sync

File-replication layer between desktop/web clients + the Task server.
Wire trait is `#[architect::rpc] trait VaultSync` in `vault-proto`.
Backend is `vault::Backend` (single + multi-tenant modes).

Picks up external edits via `Backend::start_watcher`. [[LLM Wiki]] doesn't
have this — it's local-only.

Related: [[Loro]], [[Architect]], [[Knowledge graph]].
