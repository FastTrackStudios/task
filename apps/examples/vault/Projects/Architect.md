---
title: Architect
type: entity
tags: [project, framework, rust]
sources: ["architect-design-md"]
priority: 1
folder: "[[Projects]]"
---

# Architect

FastTrackStudio's Rust RPC framework. `#[architect::rpc]` macro turns a sync
trait into: the sync trait itself + async client + server bridge through a
`Dispatcher`. Task uses it for [[Vault sync]] + every other service.

Same shape powers [[Daw]] (REAPER extension on a different backend).

Related: [[Vox]], [[Daw]].
