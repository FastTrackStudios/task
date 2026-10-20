# 6. Pure peer-to-peer nodes over agnostic storage

Date: 2026-10-20

## Status

**Proposed** — drafted by Commander for the Captain's review. Not accepted
until the Captain says so. Once accepted, this ADR is a hard constraint:
any design, spec, or code that conflicts with it is a bug to be fixed, not
a trade-off to be argued.

Builds on and tightens: `docs/spec/offline.md` (`offline.local`,
`offline.peers`, `offline.account`, `offline.server-peer`),
`features/files/spec/files.md` (`files.scale.capacity`,
`files.peering.scale`), the `CONTEXT.md` glossary (Storage Location,
Storage agent, File Root), and ADRs 0001, 0003 and 0005.

## Context

Most of the pieces already point this way: every native install runs the
backend in-process, nodes sync documents over iroh with "no hub", a
personal server is "one more device", and Files is designed so capacity
grows by attaching storage locations rather than replacing a machine.

What has been missing is a single rule that settles arguments. Without
one, server-only conveniences accrete one at a time (an endpoint only the
host exposes, a job only the host runs, a storage kind only `task-server`
can speak for), and each is reasonable on its own. Together they rebuild a
central server. Likewise, storage support grows kind by kind, and each kind
adds its own special path.

## Decision

### 1. Every node is a full node

- **Capability parity.** Anything one node can do, every node can do:
  serve the API, hold and serve documents and File Roots it is placed on,
  run sync, transcode, index, search, host shares and review pages, accept
  bot/MCP calls, act as a storage agent. The only differences between
  nodes are **placement** (what data is on them), **uptime**, and
  **reachability**. They are never differences in **code path or feature
  set**.
- **A "server" is a role, not a kind.** It is a node configured to be
  always on, publicly reachable, and placed with a large share of the data
  (often a complete copy for durability, `files.peering.backup`). It runs
  the same binary surface as a desktop node. There is no feature you need
  "the server" for.
- **No hub in the data path.** Nodes sync directly with each other. A node
  that is down or gone never blocks two other nodes from converging. Data
  never has to pass through an always-on host to get between peers.
- **Bootstrap is not authority.** Directories that help peers find each
  other (today `fts-auth`'s device address book, `offline.account`) are
  optimizations. They hold names and endpoint ids, never content, and they
  must have a fallback (LAN discovery, manual pairing, an existing peer's
  address book) so that losing the directory never stops already-admitted
  peers from working.
- **The web UI is the one pure client** (as `offline.peers` already says).
  It talks to whichever node serves it. It is not a node and doesn't need
  to be.
- **Access is enforced by every node** per document and per root against
  org membership (`offline.doc.access`). No node is trusted because of
  which machine it runs on.

### 2. Storage is agnostic and pooled

- **One logical store, many backends.** An org's storage is the sum of its
  Storage Locations. A location can be a local disk, an external SSD, any
  number of NAS boxes, or S3-compatible object storage. Above the
  storage-agent protocol, Task code never branches on which backend holds
  the bytes.
- **Paths never encode location.** A path resolves to content, and content
  resolves to locations (`files.scale.capacity`). Adding, removing, or
  rebalancing a location moves bytes, never paths, links, or ContentRefs.
- **Capability classes, not product names.** A location declares what it
  can do (`LiveTrees`, `Blobs`) and its health
  (`Online` / `Offline` / `ExpectedOffline`). Policy reads those
  declarations, never the vendor. An object store being blobs-only is a
  declared capability, not a special case scattered through callers.
- **Any node can speak for storage.** The storage-agent protocol is the
  same whether the agent runs in-process in a node, in the desktop app for
  a plugged-in drive, or standalone on a NAS (`AgentHosting`). Any node,
  not just the always-on host, can host an agent and announce locations.
- **Placement is policy, not plumbing.** Replication count, "complete
  copy" durability hosts, selective sync slices, and hot/cold tiers are
  declared on roots and grants and are re-evaluated when locations change.
  Redundancy *below* a location (RAID, ZFS, an SSD cache) is the
  substrate's business.
- **Expected-offline is normal.** Drives and NAS boxes come and go. A
  missing location degrades availability of the bytes placed only there;
  it never corrupts state and never blocks writes elsewhere.

## Rules for every change (the guardrail checklist)

A change conflicts with this ADR, and must be redesigned or explicitly
excepted by the Captain, if any answer is "yes":

1. Does it add an RPC, job, or UI action that only an always-on host (or
   only `task-server`) can perform?
2. Does it require a specific node to be online for two other nodes to
   sync, share, or review?
3. Does it make a directory or auth service a source of truth for content
   or membership that peers can't verify or continue without?
4. Does it branch on a storage backend type (NAS vs S3 vs disk) above the
   storage-agent protocol?
5. Does it put a location, host name, or mount path into a durable
   identifier (path, link, ContentRef, manifest)?
6. Does it assume one location holds everything, or that capacity is
   bounded by one machine?

Every such feature also gets an integration scenario that proves parity:
the same behavior with the always-on host **absent** (two device nodes
only), and storage spread across **at least two locations of different
kinds**.

## Known tensions to resolve (not settled by this ADR)

These are current-tree facts that look like they conflict. Each needs a
ticket or an explicit, recorded exception:

- `CONTEXT.md` "Storage Location" says locations are deployment-scoped and
  the operator registers them; "Storage agent" says agents announce and
  the operator approves. In a peer world, who approves an agent, and where
  is the admitted set kept so no single host is required?
- `files-storage-proto` documents that only `AgentHosting::InServer` is
  implemented; the desktop and standalone NAS agents are the same protocol
  over the wire but are not yet built.
- `LocationKind::ObjectStore` (S3-compatible) exists in the model. Whether
  a blob store backend is implemented and tested end to end has not been
  verified.
- `offline.account` uses `fts-auth` for device discovery and personal-org
  joining. The fallback path when the issuer is unreachable needs to be
  stated and tested.
- `offline.doc.access` says the *server* checks access per document on
  every sync. Under this ADR, every node checks. The wording and tests
  should match.
- Some features may still exist only on the host (for example, public
  share or review serving, the transcoder, and webhooks). These need an
  inventory before they can be counted as parity gaps.

## Consequences

- Specs and tests gain a "host-absent" and a "mixed-storage" axis. That is
  more test surface, but it is also what makes the claim true.
- Some existing server-only shortcuts become debt with tickets against
  them.
- Operating a fleet gets simpler. The Captain's server, NAS boxes,
  workstations, and laptops are the same kind of thing. Adding a NAS or an
  S3 bucket adds capacity. Losing a machine loses only what was placed
  solely on it, and durability policy is what prevents that.
