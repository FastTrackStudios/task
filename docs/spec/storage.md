# Storage Spec

Where state lives, and which representation is authoritative when more than one
holds the same fact. Cross-cutting: the vault, Files, projects, finance and
timers all answer to it.

The boundary is **not** markdown versus database. It is what kind of state a
fact is:

| Tier | What it is | Where it lives |
|---|---|---|
| **Authored** | a human typed it | the vault, as markdown |
| **Observed** | the world told us | a database |
| **Derived** | we computed it | a database, rebuildable |

Answers to charters
[#2 A Single Trusted System](../charter/02-single-trusted-system.md)
and [#8 The Ideal Vault (PKM)](../charter/08-vault.md).

---

## Tiers

### Authored state is markdown, and stays legible without us

t[storage.tier.authored]
Anything a human wrote — notes, project declarations, task text, charts — lives
in the vault as markdown with YAML frontmatter, and carries enough context to be
interpreted with nothing but the file. It is greppable, `cp -r`-able and
editable in any editor while this application is not running.

---

### Observed state is a database, never markdown

t[storage.tier.observed]
Machine-observed facts — file catalogue entries, sizes, content addresses,
timestamps, sync and hydration state, device registrations, access logs — live
in a database. They are high-volume, high-churn and not authored by anyone, and
putting them in markdown buys no portability that anyone can use.

---

### Derived state is a database, and is disposable

t[storage.tier.derived]
Anything computed — indexes, rollups, aggregates, extracted text, thumbnails —
lives in a database and is reconstructible from its inputs. Derived state is
never the only copy of anything, and deleting it costs time to rebuild and
nothing else. Where a derived artifact must also be portable, it is additionally
written beside its source (see `files.index.portable`).

---

## Projections

### Delete every database and lose nothing a human wrote

t[storage.projection.rebuildable]
Removing every database and restarting reconstructs a fully working system from
the vault tree and the content store alone. Nothing authored is lost, and the
only cost is the time to re-index. This is the test that keeps the portability
promise honest: a projection that cannot be rebuilt has silently become a source
of truth.

---

### A write lands in the file first

t[storage.projection.write-through]
A change to authored state writes the file and updates the projection as one
operation, and the file is what is durable: a crash between the two leaves the
file correct and the projection stale, never the reverse. A projection that
disagrees with its file is wrong by definition and is repaired from the file.

---

### Outside edits are first-class

t[storage.projection.external-edits]
A file changed by another tool — an editor, a sync client, a shell — is detected
and re-projected without restarting, and is not treated as a conflict merely for
having originated outside. The vault having other writers is the normal case,
not an error path.

---

## Concurrency

### Concurrent structured state uses architect-crdt

t[storage.crdt.layer]
Structured state edited concurrently — catalogue entries, project structure,
task state — merges through `architect-crdt`, over Loro. It is the merge layer,
not a third source of truth: authored facts still resolve to their file, and a
document is a means of converging on what the file will say, never a competing
record of what it already says.

---

### A whole-document save names the revision it was made against

t[storage.save.revision]
A chart or a song read through the library lane carries a server-issued
revision: a digest of what the document says — its frontmatter as data, keys
in sorted order, then its body — so it means the same thing on every node
holding the document, including after the collaborative layer re-renders the
file with its keys in another order. A
save that sends a revision lands only if the document is still at it; anything
else — another client's save, a collaborative edit flushed, a change pulled
from another device, the document deleted — is refused as `Stale` with the
revision it is at now, and nothing is written. The check and the write are one
step on the node. A save with no revision is unconditional, for creates and
for clients written before revisions existed.

---

### A whole-list save of a collection is conditional too

t[storage.save.collection]
A collection (a setlist, a show, a library) carries a revision the store
issues — one on create, one more on every change of any kind. A whole-list
save (`CollectionService::save`: the title and the full order) names the
revision it was read at and is refused as `Stale` when the collection has
moved on, writing nothing. Single-item edits (`add_item`, `remove_item`,
`reorder`) stay unconditional because two of them compose rather than one
erasing the other; they still move the revision on.

---

## Query

### No query answers by scanning

t[storage.query.no-scan]
No read path answers a query by walking the tree or by parsing every entity of a
type. Lookups by identity are constant-time, and queries over a collection are
served by an index. Query cost scales with the size of the result, not with the
size of the vault.

---

### Every query surface reaches the browser

t[storage.query.reach]
Every query a client makes is answerable from a browser — either executed
locally against a resident subset, or served by the server over vox. No read
path may depend on a database engine that exists only on native platforms, and
the same API shape serves both.

---

## Open decisions

**Where the projection database lives per platform.** SQLite per org is the
native answer and already in use. In WASM, SQLite over OPFS is possible but
heavy; a resident catalogue slice with server-side queries is lighter.
`storage.query.reach` requires that the choice not leak into the API; which
choice it is remains open.

**What `architect-crdt` must gain.** It is the intended merge layer per
`storage.crdt.layer` and does not yet cover the shapes here — large ordered
trees, per-entry staleness, and partial replication of a subtree. Scoping that
work is not yet done.
