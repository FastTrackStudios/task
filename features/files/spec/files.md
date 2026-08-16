# Files Spec

The testable form of charter [#3 The Ideal File Manager](https://github.com/FastTrackStudios/task/issues/3).
The charter is the high-level map — 25 hard requirements, `R1`–`R25`. This spec
is the detail each one is judged against, and every rule below names the charter
requirement it descends from.

The design of record is [`../docs/nextcloud-parity.md`](../docs/nextcloud-parity.md).

---

## Scale

### t[files.scale.large-media]
_(R1)_ A file of arbitrary size is uploaded, versioned, synced and read without
being held in memory in full by any process, and without a reader waiting for a
complete transfer. Content is chunked on the way in (FastCDC, BLAKE3-addressed)
and every read path is a stream. Byte-range reads are served from the chunks
covering the range only: seeking to the middle of a multi-gigabyte video fetches
the chunks under the playhead, never the whole file. Progress and cancellation
are observable for any transfer, and a cancelled or interrupted transfer leaves
no partial file visible in a root's live tree.

---

### t[files.scale.small-files]
_(R2)_ Operations over a directory are proportional to the directory, not to its
files: listing a folder of ten thousand files is one request returning one
response, not ten thousand. Sync reconciles a subtree by comparing tree state,
not by statting each file individually. Identical content is stored once
regardless of how many paths or roots reference it, and adding a second
reference to existing content transfers no bytes.

---

### t[files.scale.capacity]
_(R3)_ Storage capacity is extended by registering an additional storage
location. Registering one requires no downtime, no migration of existing
content, and no change to any file's path — a path resolves to content, and
content resolves to a location, so the two vary independently. Which location
holds a given file is a policy decision evaluated from the file's project type
and facet, and re-evaluating that policy relocates content without the path
changing.

---

## Manipulation

### t[files.write.surface]
_(R4)_ `FilesService` exposes `mkdir`, `rename`, `move`, `copy` and `delete` as
RPC methods reachable by every client over vox. Each is transactional: it
completes wholly or leaves the tree untouched, and each is wrapped in exactly one
jj operation so history shows the user's action rather than its constituent
writes. Each accepts a set of paths as readily as one, applying atomically
across the set. A selection can be retrieved as a single archive stream without
first materialising it on the server.

⚠️ Each new method requires its `permits.rs` row in the same change, or it fails
closed in production.

---

### t[files.write.upload]
_(R5)_ Upload is chunked and resumable: an interrupted upload resumes by
transferring only the chunks not already present, and content already in the
store is not re-uploaded at all. When an upload targets an existing path, the
conflict is surfaced with three outcomes — keep both, replace, keep existing —
and no outcome is chosen automatically. `replace` records a new version rather
than discarding the old (see `t[files.version.keep-both]`).

---

## Liveness

### t[files.live.propagation]
_(R6)_ A structural change — create, rename, move, copy, delete — is reflected
in the originating client's view before the server acknowledges it, and is
reverted visibly if the server rejects it. Every other subscribed client
receives the change as a `FilesEvent` over vox and updates without a refresh or
a poll. A client that reconnects after a gap converges to current tree state
without a full re-listing.

---

## Placement

### t[files.sync.selective]
_(R7)_ A device subscribes to facets of a project rather than to path globs;
the facet vocabulary is a property of the project type. Unsubscribed content is
present in the live tree as a dehydrated pointer stub with correct name, size
and metadata, and is hydrated on access. A facet declares atomicity, and
subscribing to an atomic facet implies its dependencies — subscribing to a
session brings the media that session references, because a session whose audio
streams in on first access will glitch.

---

## Topology

### t[files.topology.multi-server]
_(R8)_ More than one server may hold an org's content, and a client resolves and
reaches content across them as one namespace. No single server is required to be
in the path of a transfer: where two peers can reach each other directly, bytes
move over iroh/QUIC between them. Server unavailability degrades reach, never
correctness — content held locally stays readable and writable.

---

## Findability

The query surface belongs to charter
[#17](https://github.com/FastTrackStudios/task/issues/17). These rules are what
Files owes it.

### t[files.index.extraction]
_(R9)_ Content is extracted automatically as bytes arrive, incrementally and
without user action: text from documents and PDFs, speech transcription from
audio and video, and a description of visual content in a shot. Extraction is
resumable and re-runnable, and re-running it on unchanged content is a no-op.
Extraction failure for one file degrades that file's findability and never
blocks its storage, sync or playback.

---

### t[files.index.local]
_(R10)_ Extraction runs on hardware the operator controls, using open models, on
the machine holding the bytes. A third-party service may be configured as an
addition and must never be a prerequisite: with no external credential
configured, every rule in this section is still satisfied. Content is never
transmitted to a third party as a side effect of becoming searchable.

---

### t[files.index.regions]
_(R11)_ An extracted result addresses a region of a file — a time range in
audio or video, a page or rectangle in a PDF, a block in a note — not the file
as a whole. A region is expressible as a stable link that opens the file at that
region. Region addressing is one scheme shared with review annotations and
resource annotations rather than a scheme private to search.

---

### t[files.index.portable]
_(R12)_ Transcripts, descriptions and extracted metadata are written as ordinary
files alongside the content they describe, in a documented plain-text format
readable without this application. They are derived artifacts: deleting them
loses no information that cannot be regenerated from the source, and they are
never the record of anything a user authored.

---

## Access

### t[files.access.internal-sharing]
_(R13)_ A file or folder is grantable to a principal within the org — a person
or a team — with an explicit capability set, without minting a public link. The
grant is an architect `Rule { resource, actions }`; a role is a named bundle of
rules and never a primitive the backend checks directly. Granting, listing and
revoking grants are RPC methods, and revocation takes effect on the next
request.

---

### t[files.access.granularity]
_(R14)_ Authorisation is evaluated at the path being acted on, via a direct
`engine.check` in the backend, and not solely at the root. A grant on a folder
conveys nothing about its parents. A principal with access to a subtree can
list, read and act within exactly that subtree; paths outside it are absent
rather than forbidden-but-visible.

---

## Concurrency

### t[files.concurrency.advisory-lock]
_(R15)_ When a principal begins editing a file whose format cannot be merged,
that fact is published and other clients surface it before an edit begins. The
signal is advisory only: it does not reject a write, does not gate an RPC, and
does not require release — a client that disappears leaves a signal that expires
on its own. Offline writes are never refused on the basis of such a signal;
collisions are resolved by `t[files.version.keep-both]`.

---

## Devices

### t[files.device.control]
_(R16)_ A device can pin a path for offline availability independently of its
facet subscriptions, and pinning survives restart. Transfers are throttleable to
a configured rate, and are pausable and resumable without loss of progress. A
device registration can be revoked; a revoked device is refused further sync and
destroys its local copy of org content on next contact.

---

### t[files.device.ingest]
_(R17)_ A registered device can be configured to upload content it originates —
camera roll, stills, voice memos — automatically to a configured destination,
without a per-item user action. Ingest is idempotent: the same item captured
once is uploaded once, across restarts and re-registrations.

---

## Organisation and accountability

### t[files.organise.manual]
_(R18)_ A file may carry user-assigned tags and a per-principal favourite flag,
both independent of extracted metadata. A tag produces a view over matching
files and never a folder membership: tagging does not move a file, and a file's
path is unchanged by any tagging operation.

---

### t[files.organise.activity]
_(R19)_ Every structural change and every access to shared content is recorded
with actor, action, target and time, and is readable by a human as a feed scoped
to a file, a folder or a root. The record is derived from the same events that
drive `t[files.live.propagation]`, so a change that propagates is a change that
is recorded.

---

## Handoff

### t[files.handoff.editor]
_(R20)_ A selection of files, or a set of region results from
`t[files.index.regions]`, can be delivered into an editing application as a bin
or a timeline — content in place, not copies. Delivery preserves region bounds:
a result covering seconds 40–52 of a clip arrives as that range, not as the
whole clip.

---

## Version control

### t[files.version.unit]
_(R21)_ The unit of version history is what the application opens, which is not
always one file: a Reaper `.rpp` is a single plain-text file, a Logic project is
a directory, a Pro Tools session is a folder of files, and a DaVinci Resolve
project is a row in a database that becomes a file only on export. Each
supported project type declares its unit, and history, restore and divergence
all address that unit. The user is shown versions of the project, never versions
of the files composing it.

---

### t[files.version.keep-both]
_(R22)_ Concurrent edits to a unit that cannot be merged both survive. Where an
offline edit and a live edit diverge, reconciliation produces two versions,
each attributed and each independently openable — never an overwrite, and never
a synthesised merge of a format we do not understand. The divergence is
surfaced to a human with the option to keep one, keep both, or keep both under
chosen names; taking no action leaves both intact.

---

### t[files.version.restore]
_(R23)_ Restoring a past version yields a unit the originating application
opens, with its media references resolvable. Where referenced content has since
been renamed, moved or consolidated, resolution follows the recorded rename
history (jj `CopyHistory`) and the content address rather than the stale path.
Restoration is non-destructive: it produces a new version and does not discard
the state restored from.

---

### t[files.version.native]
_(R24)_ Each project type declares the artifacts its application generates for
its own purposes — auto-backups, `.rpp-bak` files, peak and waveform caches,
render caches, application-internal project versions. These are recognised and
either excluded from history or folded into it, and are never presented as
user-facing versions. Excluding them does not exclude them from storage where
the user has chosen to sync them.

---

### t[files.version.cadence]
_(R25)_ Checkpoints are created automatically at resting points — quiescence
after writing stops, plus save points — and never on every write. Any version,
automatic or manual, can be given a name after the fact, and a named version is
exempt from retention-driven collection. The vault root carries its own cadence
profile: `IfMatch`/sha remains the editor's per-save concurrency check and Loro
remains the live multiplayer layer, with jj checkpointing periodically above
both.
