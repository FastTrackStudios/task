---
name: architecture-guardrails
description: Check a design, spec, ticket, or diff against Task's two hard goals before building it — every node is a full peer (servers are just always-on peers), and storage is one pooled system across disks, NAS boxes and S3. Use before writing a spec or ADR, before implementing any feature that touches sync, sharing, review, auth, Files or storage, and when reviewing a PR.
runs_as: any agent working in this repo
trigger: on-demand (design, implementation, review)
---

# Architecture guardrails

Task has two non-negotiable goals. The full statement is
[`docs/adr/0006-pure-peer-to-peer-and-agnostic-storage.md`](../docs/adr/0006-pure-peer-to-peer-and-agnostic-storage.md).
Read it once. This skill is the working checklist.

1. **Pure peer to peer.** Every node (desktop, phone, a server, a NAS
   running the agent) has the full functionality. A "server" is just a
   peer that is always on, publicly reachable, and placed with a lot of
   data. Nothing needs a central server.
2. **Agnostic storage.** Local disks, external SSDs, any number of NAS
   boxes, and S3-compatible buckets are one cohesive store. Code above the
   storage-agent protocol never knows or cares which one holds the bytes.

## Before you design or build

Answer each question in your ticket, PR, or handoff. Any "yes" means
redesign, or stop and ask the Captain for an explicit, written exception.

| # | Question | Typical fix |
|---|---|---|
| 1 | Does this add an RPC, job, or UI action only an always-on host (or only `task-server`) can do? | Put it in the node backend that every install runs (`offline.local`). Gate it on placement or permission, never on node type. |
| 2 | Must a particular node be online for two *other* nodes to sync, share, or review? | Sync directly between the nodes holding the data (`offline.peers`). |
| 3 | Does a directory or auth service become the source of truth for content or membership? | Keep it to names and endpoint ids, and give peers a fallback (`offline.account`). |
| 4 | Does code branch on storage backend type (disk / NAS / S3) above the storage-agent protocol? | Branch on declared `CapabilityClass` and `LocationHealth` instead. |
| 5 | Does a host name, mount path, or location id end up in a path, link, ContentRef, or manifest? | Resolve path → content → location at read time (`files.scale.capacity`). |
| 6 | Does it assume one location or one machine holds everything? | Use placement policy plus selective sync. Hold a complete copy only where durability asks for it. |
| 7 | Does an offline or unplugged location surface as an error or block writes elsewhere? | Treat it as `ExpectedOffline` health and degrade only the affected bytes. |

## Proving it

The repo already requires an integration scenario and a seeded example for
every feature (`CLAUDE.md`, "everything lives in the suite and the seed").
For anything in scope here, the scenario also has to show:

- **Host absent.** The behavior works between two device nodes with the
  always-on host stopped. The host can join later and converge.
- **Mixed storage.** The data sits on at least two locations of
  *different kinds* (for example a live-tree volume and a blobs-only
  object store, or a removable drive that goes `ExpectedOffline`
  mid-scenario). Reads, writes, and placement changes still behave.

If you can't write those two stages yet, say so in the PR and link the
ticket. Don't claim the feature meets these goals.

## When you find an existing violation

Don't fix it silently inside an unrelated change. File it, or comment on
the relevant spec rule (`t[...]` marker), with:

- the guardrail question number;
- the file and line;
- what peer-parity or storage-agnostic behavior is missing.

ADR 0006 lists the known tensions so far, under "Known tensions to
resolve".

## Decision boundaries

- **Decide yourself:** how to satisfy a guardrail inside your own change.
- **Ask the Captain:** any exception to a guardrail, any change to
  ADR 0006, anything that makes an always-on host or an auth directory
  required, and anything that adds a backend-specific code path.
