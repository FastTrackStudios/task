---
title: CRDT
type: concept
tags: [crdt, distributed-systems, sync]
sources: ["loro-spec", "yjs-paper-2020"]
status: stable
folder: "[[Wiki]]"
---

# CRDT

A **conflict-free replicated data type** is a data structure whose state can be
modified concurrently on multiple replicas and merged deterministically. The
field is dominated by two families: **state-based** (CvRDT) and
**operation-based** (CmRDT).

Task uses [[Loro]] — an op-based CRDT library tuned for collaborative text +
trees. The wire trait that ships updates between peers lives in
`features/project/project-proto/src/sync.rs` and is documented in [[Vault sync]].

Related: [[Yjs]], [[Automerge]], [[Operational transformation]], [[Loro]].

Read more in [[Building local-first software]]^crdt-pillar.
