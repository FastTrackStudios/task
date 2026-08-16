# Files Spec

The testable form of charter [#3 The Ideal File Manager](https://github.com/FastTrackStudios/task/issues/3),
which maps these requirements at one line each. Design of record:
[`../docs/nextcloud-parity.md`](../docs/nextcloud-parity.md).

Reference a requirement by id — `files.scale.large-media` — in issues, commits
and code (`t[impl files.scale.large-media]`).

---

## Scale

### Large media is first-class

t[files.scale.large-media]
Files of any size stream both ways: chunked on write, range-read on read, never
held whole in memory. Seeking mid-file fetches only the chunks under the
playhead. Transfers report progress, cancel cleanly, and leave no partial file
in a live tree.

---

### Small files at volume stay cheap

t[files.scale.small-files]
Directory operations are proportional to the directory, not to its file count —
listing ten thousand files is one request. Sync compares tree state rather than
statting per file. Identical content is stored once, and re-referencing it
transfers nothing.

---

### Capacity expands by attachment

t[files.scale.capacity]
Capacity grows by registering a storage location: no downtime, no migration, no
path changes. A path resolves to content and content resolves to a location, so
re-evaluating placement policy moves bytes without moving paths.

---

## Manipulation

### The full write surface works over the network

t[files.write.surface]
`FilesService` exposes `mkdir`, `rename`, `move`, `copy` and `delete` over vox.
Each is transactional and wrapped in one jj operation, so history records the
action rather than its constituent writes. Each takes a set of paths as readily
as one, and a selection downloads as a single archive stream.

⚠️ Every new method needs its `permits.rs` row in the same change, or it fails
closed in production.

---

### Uploads survive reality

t[files.write.upload]
Uploads are chunked and resumable: an interrupted one sends only the missing
chunks, and content already in the store sends nothing at all. A collision
offers keep-both, replace or keep-existing and never picks for you; `replace`
records a new version rather than discarding the old.

---

## Adoption

Most content does not arrive through an upload. It is already on disk, written
by the applications that made it, and has to become ours without ceasing to be
theirs.

### A tree is adopted where it lies

t[files.adopt.in-place]
Adopting an existing tree as a root moves, copies and renames nothing. The
applications already reading and writing that tree keep doing so throughout, and
adoption never requires quiescing them or taking the tree offline.

---

### Structure first, content addresses later

t[files.adopt.catalogue-first]
Adoption publishes catalogue entries from what the filesystem already knows —
name, size, timestamps — so a tree is browsable within seconds of starting
whatever its size, long before its bytes have been read. Content addresses are
computed in the background, and an entry whose address is not yet known is
marked unverified rather than withheld.

---

### Adoption resumes, and tolerates writers

t[files.adopt.resumable]
Interrupting adoption loses only the work in flight; resuming continues rather
than restarting. A file modified while it is being hashed is re-hashed rather
than recorded wrongly, and no application is ever blocked so that adoption can
finish.

---

## Facets

### A facet is a named class of content

t[files.facet.vocabulary]
A facet names a class of content a project holds — sessions, stems, mixes,
footage, proxies, renders. Facets are the unit of both selective sync
(`files.sync.selective`) and placement policy (`files.scale.capacity`): one
vocabulary serves both, and a capability declares the facets its work produces.

---

### Tool layouts belong to the capability

t[files.facet.tool-layout]
Where an application always produces the same layout, that mapping belongs to
the capability rather than to any project. Pro Tools always writes `Audio
Files`, `Bounced Files` and `Session File Backups`; Reaper always writes `Media`
and `Backups`. Any project holding the capability recognises them with no
configuration.

---

### A project maps what its tools did not create

t[files.facet.project-override]
Directories that reflect a project's own conventions rather than a tool's are
mapped to facets by the project. An unmapped directory belongs to no facet,
syncs with the project's default, and is reported so the mapping can be
supplied. It is never hidden, and its facet is never guessed.

---

## Ignoring

### Ignoring has two independent layers

t[files.ignore.layers]
A platform layer covers what an operating system leaves behind — `.DS_Store`,
AppleDouble `._*` files, thumbnail caches — and applies everywhere regardless of
capability. A capability layer covers what an application leaves behind (see
`files.version.native`). Neither layer can be defeated by the other, and a file
matching either is ignored.

---

### Ignored is not deleted

t[files.ignore.retained]
An ignored file is absent from user-facing listings and from history, and
remains present on disk and synced wherever the user chose to sync the directory
holding it. Ignoring changes what is shown and versioned, never what exists.

---

## Liveness

### Changes appear instantly, everywhere

t[files.live.propagation]
A structural change renders on the originating client before the server
acknowledges it, and visibly reverts if rejected. Other clients receive it as a
`FilesEvent` and update without polling. A client reconnecting after a gap
converges without re-listing the tree.

---

## Placement

### Selective local sync, by project type

t[files.sync.selective]
Devices subscribe to a project type's facets, not to path globs. Unsubscribed
content stays present as a dehydrated stub with real name, size and metadata,
hydrating on access. Atomic facets bring their dependencies — a session arrives
with the media it references, because one that streams in on first play will
glitch.

---

## Topology

### More than one server

t[files.topology.multi-server]
An org's content may span several servers and resolves across them as one
namespace. Where two peers can reach each other, bytes move directly over
iroh/QUIC. A server being unreachable costs reach, never correctness: local
content stays readable and writable.

---

### Files federate across servers

t[files.topology.federation]
A file or folder is shareable to a principal on another server and arrives there
as a first-class item — browsable, versioned, syncable, pinnable — not a
download link. Grants carry across the boundary with their capabilities intact
and stay revocable from the originating side. Content addressing is shared, so
bytes already held on the receiving server transfer again as nothing. A remote
server going away costs access to its own content and nothing else: local
content and local history are untouched.

The federation model — identity, addressing, and who grants trust between
servers — belongs to charter
[#22](https://github.com/FastTrackStudios/task/issues/22); this rule is what
Files does once that boundary exists.

---

## Catalogue

Structure replicates everywhere; content replicates selectively. The catalogue
is the replicated structure — what exists, of what kind, of what size, held
where — and it is what makes a tree spanning several servers browsable at all.
It is authoritative about structure and about nothing else.

### The catalogue covers everything reachable

t[files.catalogue.complete]
Every file and folder the principal may reach has a catalogue entry, whatever
its hydration state and whichever server or location holds it. An entry carries
name, path, kind, size, content address, current version, the locations holding
it, and hydration state. It does not carry content, renditions or extracted
text — those are heavier and fetched on demand.

---

### The tree browses with no network

t[files.catalogue.offline]
The full tree — including content on servers currently unreachable — lists,
sorts, opens to metadata and resolves links with the network entirely absent.
What cannot be fetched is visibly unavailable rather than missing: a folder
never appears empty because its server is down.

---

### The catalogue scales with file count, not bytes

t[files.catalogue.bounded]
Catalogue size is proportional to the number of files, never to their size, and
stays small enough for a phone to hold a working set — an order of tens of
megabytes for a tree of a hundred thousand files. A client may hold the
catalogue for a subtree rather than the whole org, and holding a subtree
constrains what it can browse, never what it can be told exists.

---

### Staleness is visible, never silent

t[files.catalogue.staleness]
Every entry records when its structure was last confirmed against its holding
location. A client can tell what it is looking at as of when, and a view over a
location it has not heard from says so. The catalogue is allowed to be stale; it
is not allowed to look current when it is not.

---

### Structure merges; it is not last-writer-wins

t[files.catalogue.concurrent]
Structural changes made offline — create, rename, move, delete — converge with
changes made concurrently elsewhere rather than one silently overwriting the
other. Where intent genuinely conflicts, both are preserved and surfaced (see
`files.version.keep-both`). Reconnecting requires no user action and no
re-listing of the tree.

---

## Findability

The query surface belongs to charter
[#17](https://github.com/FastTrackStudios/task/issues/17). These are what Files
owes it.

### Content is extracted, not just filenames

t[files.index.extraction]
Content extracts as bytes arrive — document and PDF text, speech transcription,
visual description of shots. Extraction is incremental, resumable, and a no-op
on unchanged content. Failure costs one file its findability and never blocks
storage, sync or playback.

---

### Extraction runs on hardware you own

t[files.index.local]
Extraction runs on operator hardware, using open models, on the machine holding
the bytes. External services are configurable additions, never prerequisites:
with no credential set, every rule here still holds. Nothing leaves for a third
party as a side effect of becoming searchable.

---

### A result is a region, not a file

t[files.index.regions]
A result addresses a region — a time range, a page, a block — and is expressible
as a link that opens there. Region addressing is one scheme shared with review
and resource annotations, not a private one for search.

---

### Derived indexes are ordinary, portable files

t[files.index.portable]
Transcripts, descriptions and extracted metadata are plain files beside the
content they describe, in a documented format readable without this app. They
are derived: deleting them loses nothing unrecoverable, and they never hold
anything a user authored.

---

## Access

### Sharing works inside the org

t[files.access.internal-sharing]
A file or folder is grantable to a person or a team with an explicit capability
set, without minting a public link. The grant is an architect
`Rule { resource, actions }`; roles are named bundles of rules, never something
the backend checks directly. Grant, list and revoke are RPC methods, and
revocation binds on the next request.

---

### Permissions are per-folder and per-file

t[files.access.granularity]
Authorisation is checked at the path being acted on, via a direct
`engine.check`, not only at the root. A grant on a folder says nothing about its
parents. Outside a granted subtree, paths are absent rather than
visible-but-forbidden.

---

## Concurrency

### What cannot be merged is flagged, not refused

t[files.concurrency.advisory-lock]
Opening a file whose format cannot be merged publishes that fact, and other
clients surface it before an edit begins. The signal is advisory: it gates no
RPC, rejects no write, needs no release, and expires on its own when a client
vanishes. Collisions resolve through `files.version.keep-both`.

---

## Devices

### Availability, transfer and revocation are controllable

t[files.device.control]
A device pins paths for offline use independently of its facet subscriptions,
and pinning survives restart. Transfers throttle, pause and resume without
losing progress. A revoked device is refused sync and destroys its local copy of
org content on next contact.

---

### Devices ingest on their own

t[files.device.ingest]
A device uploads what it originates — camera roll, stills, voice memos — to a
configured destination with no per-item action. Ingest is idempotent across
restarts and re-registration: captured once, uploaded once.

---

## Organisation and accountability

### Hand-organisation, as views

t[files.organise.manual]
Files carry user-assigned tags and a per-principal favourite flag, independent
of extracted metadata. A tag produces a view, never folder membership — tagging
never moves a file or changes its path.

---

### Every change is attributable

t[files.organise.activity]
Every structural change, and every access to shared content, records actor,
action, target and time, readable as a feed scoped to a file, a folder or a
root. It derives from the events that drive `files.live.propagation`, so what
propagates is what is recorded.

---

## Handoff

### Results reach the tool that does the work

t[files.handoff.editor]
A selection, or a set of region results, delivers into an editing application as
a bin or a timeline — content in place, not copies. Region bounds survive the
trip: a hit covering 0:40–0:52 arrives as that range, not as the whole clip.

---

## Version control

### The versioned unit is what the application opens

t[files.version.unit]
History addresses what the application opens, which is often not one file: a
Reaper `.rpp` is one text file, a Logic project a directory, a Pro Tools session
a folder, a Resolve project a database row that becomes a file only on export.
Each project type declares its unit; users see versions of the project, never of
its parts.

---

### Divergent edits keep both

t[files.version.keep-both]
Concurrent edits to an unmergeable unit both survive. Divergence yields two
attributed, independently openable versions — never an overwrite, and never a
synthesised merge of a format we do not understand. A human keeps one, both, or
both renamed; doing nothing keeps both.

---

### A restored version still opens

t[files.version.restore]
A restored version opens in its application with media references resolvable,
following recorded rename history (jj `CopyHistory`) and content addresses
rather than stale paths. Restoring is non-destructive: it produces a new version
and discards nothing.

---

### The application's own versioning is absorbed

t[files.version.native]
Each project type declares what its application generates for itself —
auto-backups, `.rpp-bak`, peak and render caches, internal project versions.
These are recognised and either folded into history or excluded, never shown as
user-facing versions. Excluding them from history does not exclude them from
sync.

---

### Human version markers are labels, not lineage

t[files.version.labels]
Version markers people put in filenames — `2.1 Somma`, `v3`, `FINAL`, `Copy of
Copy of` — are surfaced as labels on the file and never interpreted as version
history. Ordering, ancestry and currency are never inferred from a name: a
convention we did not define is one we can only read, since the first file that
breaks the pattern would otherwise be silently misfiled.

---

### Checkpoints accrue on their own

t[files.version.cadence]
Checkpoints happen at resting points — quiescence after writes stop, plus save
points — never per write. Any version can be named after the fact, and naming
exempts it from retention collection. The vault root carries its own profile:
`IfMatch`/sha stays the editor's per-save check, Loro stays the live layer, and
jj checkpoints above both.
