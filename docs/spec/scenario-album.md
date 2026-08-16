# Scenario: the federated album

One worked scenario that exercises the whole system end to end. If this runs, the
requirements in [files](../../features/files/spec/files.md),
[project](../../features/project/spec/project.md),
[vault](../../features/vault/spec/vault.md) and [storage](storage.md) are met in
combination and not merely in isolation.

Each stage is a rule; each stage lists the requirements it exercises. The
coverage table at the end says what this scenario does **not** reach.

## The setup

Two albums, chosen because they are opposites. The first is heavy and stays
that way; the second is light and changes shape repeatedly.

### Album one — Crescendum

A ten-song worship album.

- **Two orgs on two servers.** `tombrooksmusic` runs the **studio** server and
  owns the audio work. `fasttrackstudios` runs the **post** server and owns the
  video work. Neither is a member of the other.
- **Ten songs.** Every song has its own Reaper session. **Three** of them also
  get a concert-video treatment cut in DaVinci Resolve.
- **On disk already**: 77 GB, 14,671 files, written by Reaper and Pro Tools for
  the last eight months. Nothing was created by us.
- **The client** is one person with a link and no account.

The shape this produces: seven songs stay **parts**; three are **promoted** to
subprojects because they carry their own capability and their own content on a
second server. The album is built the same way either way.

### Album two — Journey

A recital, recorded as **one continuous piano take** and later chopped into
seven pieces. 244 GB in 48 files — the mirror image of Crescendum's 77 GB in
14,671.

Nothing here starts as a project but the album itself. Its pieces are parts,
carved out of a single recording. Over the following months the album changes
shape repeatedly: one piece grows into real work and is promoted; the work is
abandoned and it is demoted; video is added to a piece and later withdrawn.

And the thing that makes it interesting: **the video company started their own
project for the concert film before anyone thought to coordinate.** Two projects
now exist for one job, on two servers, with no common ancestor. They have to
become one.

---

## Adoption

### An existing 77 GB tree is adopted in place

t[scenario.album.adopt]
The album directory is adopted as a project on the studio server without moving,
copying or renaming anything. It is browsable within seconds — long before 77 GB
has been hashed — and Reaper keeps writing to it throughout. Interrupting and
resuming adoption does not restart it. The `._*` and `.DS_Store` files that make
up much of the tree never appear as user-facing content.

_Exercises_ `project.identity.adoption` · `files.adopt.in-place` ·
`files.adopt.catalogue-first` · `files.adopt.resumable` ·
`files.ignore.layers` · `files.ignore.retained` · `files.catalogue.complete` ·
`files.catalogue.staleness` · `files.scale.small-files` ·
`storage.projection.external-edits`

---

### The album declares itself in frontmatter

t[scenario.album.declare]
A `project.md` at the album root declares `type: project`, a stable id,
`capabilities: [music-production]` and `form: album`. Its ten songs are declared
as parts. Nothing outside that file is needed to interpret it, and renaming the
directory in Finder changes nothing about the project's identity.

_Exercises_ `project.identity.declaration` · `project.identity.stable` ·
`project.form.grammar` · `project.form.closed` · `project.part.unit` ·
`storage.tier.authored` · `vault.index.lookup`

---

## Growth

### Three songs are promoted, seven are not

t[scenario.album.promote]
The three songs receiving video treatment are promoted to subprojects and gain
`video-production` alongside `music-production`. The other seven remain parts
carrying a Reaper session as a component. Every link, deliverable and time entry
attached to a promoted song continues to resolve, and nothing that referenced it
needed to know promotion happened. A promoted song is demotable.

_Exercises_ `project.part.promotion` · `project.capability.multiple` ·
`project.capability.conventions` · `project.nesting.uniform` ·
`project.form.components`

---

### The post server joins the same project

t[scenario.album.federate]
The post server holds the footage, the Resolve projects and the renders for
those three songs. They compose into the same album — one tree, one deliverable
set, one identity — with no location privileged as the album's real home.
Access is a grant on the content; nobody joins the other's org, and the album is
not duplicated per org.

Those three songs are children of the album while none of their bytes sit under
the album's directory — parentage is the declared link, not the containment.

_Exercises_ `project.location.composed` · `project.location.federated` ·
`project.nesting.explicit` · `files.topology.federation` ·
`files.topology.multi-server` · `files.access.internal-sharing`

---

### A photographer's stills arrive on their own

t[scenario.album.ingest]
Stills shot during tracking upload from the photographer's phone into the
album's inbox with no per-item action, once each, across restarts.

_Exercises_ `files.device.ingest` · `files.write.upload`

---

## Lifecycle — album two

### Seven pieces, no projects

t[scenario.piano.parts]
Journey's seven pieces are declared as parts of one album. None is a project:
none has a directory, a marker or capabilities of its own, and creating seven
projects for seven regions of a single recording is refused as ceremony. Each
piece addresses a time range of the continuous take, and later, when per-piece
audio is rendered to its own file, each piece is still a part — materialising
content changes nothing about what it is.

_Exercises_ `project.part.unit` · `project.form.grammar` ·
`files.index.regions` · `files.scale.large-media`

---

### One piece is promoted, then demoted

t[scenario.piano.promote-demote]
One piece turns out to need real work and is promoted to a subproject: its own
files, its own tasks, its own sessions. Months later the work is abandoned and
it is demoted back to a part. Through both transitions every link, deliverable,
setlist reference and time entry attached to it continues to resolve, and
nothing that referenced it needed to know which side of the line it was on.
Demotion loses no history.

_Exercises_ `project.part.promotion` · `project.identity.stable` ·
`project.nesting.uniform`

---

### Video is added to a piece, and later withdrawn

t[scenario.piano.capability-churn]
`video-production` is added to the promoted piece, bringing its facets,
conventions and surfaces to content that already exists. When the video is
dropped from the release, the capability is removed — and not one byte goes with
it. The footage stays browsable and versioned, simply without a video surface,
and re-adding the capability restores that surface over the same content.

_Exercises_ `project.capability.mutable` · `project.capability.multiple` ·
`project.capability.conventions`

---

### Two projects, started separately, become one

t[scenario.piano.merge]
The video company's concert-film project and the audio company's album project
were created independently — different orgs, different servers, no common
ancestor, neither aware of the other. They merge into one project. Capabilities
union to `music-production` + `video-production`; the film's footage and the
album's audio compose without a byte moving; parts reconcile where both named
the same piece, and a human resolves the cases where they disagree.

_Exercises_ `project.lifecycle.merge` · `project.location.composed` ·
`project.location.federated` · `project.nesting.explicit`

---

### The link the client already had still works

t[scenario.piano.merge-identity]
A share link the video company sent their client a week before the merge still
opens, and now resolves to the merged project. Tasks filed against either
original, time logged against either, and links written into notes referencing
either all continue to resolve. The merge records what it absorbed, so someone
who only ever knew the film project can see where it went.

_Exercises_ `project.lifecycle.merge-identity` · `project.identity.stable` ·
`files.access.internal-sharing`

---

## Working

### Two people subscribe to different halves of the same album

t[scenario.album.facets]
The mix engineer subscribes to sessions and stems; the video editor subscribes
to footage and proxies. Neither configured anything for the tool directories:
Pro Tools' `Audio Files` / `Bounced Files` / `Session File Backups` and Reaper's
`Media` / `Backups` are recognised from the capability alone. The album's own
`Project Assembly` and `Video ISO Files` belong to no tool, so they are reported
as unmapped and the project maps them by hand — they sync with the default in
the meantime rather than disappearing. Subscribing to a session brings the media
it references.

_Exercises_ `files.facet.vocabulary` · `files.facet.tool-layout` ·
`files.facet.project-override` · `files.sync.selective`

---

### An engineer works the album on a plane

t[scenario.album.offline]
With no network, the full album tree browses — including the three songs whose
content lives on the post server. Sizes, kinds and structure are all present;
unfetched content is visibly unavailable rather than absent, and no folder
appears empty because its server is unreachable. Pinned sessions open and edit.
The catalogue says how current it is.

_Exercises_ `files.catalogue.offline` · `files.catalogue.bounded` ·
`files.catalogue.staleness` · `files.device.control` · `files.sync.selective` ·
`project.location.degraded`

---

### Two engineers diverge on one session, and both survive

t[scenario.album.diverge]
A second engineer opening a session someone else has open is told before
starting, not after — and is not blocked. Both edit, one of them offline. On
reconnection the session has two attributed, independently openable versions;
neither overwrote the other and nothing was auto-merged. A human keeps one,
both, or both renamed.

_Exercises_ `files.concurrency.advisory-lock` · `files.version.keep-both` ·
`files.version.unit` · `files.catalogue.concurrent` · `storage.crdt.layer`

---

### Renaming a song reaches everyone at once

t[scenario.album.rename]
Renaming a song folder renders immediately for the person doing it, without
waiting on the server, and appears on every other connected client without a
refresh — including clients on the other server. A bulk move of stems applies
atomically across the selection or not at all.

_Exercises_ `files.live.propagation` · `files.write.surface` ·
`files.catalogue.concurrent` · `storage.query.no-scan`

---

### An upload collision asks

t[scenario.album.collide]
An assistant uploads a stem whose name already exists. The upload resumes across
a dropped connection, transfers only chunks not already held, and offers keep
both, replace, or keep existing. `replace` records a new version rather than
discarding the old.

_Exercises_ `files.write.upload` · `files.scale.large-media` ·
`files.version.cadence`

---

## Finding

### The spoken intro is findable by what was said

t[scenario.album.search]
Searching for a phrase spoken in a take returns the seconds it occupies inside a
two-hour recording, not the file containing it, and opening the result lands
there. Extraction ran on the studio server's own hardware with open models;
nothing was uploaded to a third party to make it searchable, and the transcripts
sit beside the media as plain files.

_Exercises_ `files.index.extraction` · `files.index.local` ·
`files.index.regions` · `files.index.portable`

---

### Results land in Resolve

t[scenario.album.handoff]
A set of region results is delivered into Resolve as a bin, content in place
rather than copied, with region bounds intact — a hit covering 0:40–0:52 arrives
as that range.

_Exercises_ `files.handoff.editor`

---

### The album is organised by hand

t[scenario.album.organise]
Takes tagged `keeper` appear in a view without moving on disk, and a favourite
is per-person. The album's activity feed shows who renamed, uploaded and deleted
what, and when.

_Exercises_ `files.organise.manual` · `files.organise.activity`

---

## Delivering

### The client sees deliverables, never the tree

t[scenario.album.deliver]
The album declares five deliverables — album master audio, per-song audio,
per-song video, promotional material, and short-form clips — not forty files
named individually. Per-song deliverables cover all ten songs for audio and the
three finished songs for video, and stay in step when an eleventh song is added.

_Exercises_ `project.deliverable.kind` · `project.deliverable.scope`

---

### One link, no account, nothing internal

t[scenario.album.client-link]
The client opens a link and reaches any deliverable in one obvious path,
organised by scope and medium. Sessions, stems, session backups and anything
marked internal are unreachable and unlisted. With download withheld they stream
a proxy, and their timestamped comments land against the version they watched.
Revoking the link takes effect on their next request.

_Exercises_ `project.deliverable.client-view` · `files.access.granularity` ·
`files.access.internal-sharing`

---

### The band takes it on tour

t[scenario.album.setlist]
A `live-set` project references the album's songs — promoted and unpromoted
alike — and a setlist is assembled by reference. Reordering it changes no
project, and a song appears in several setlists at once.

_Exercises_ `project.setlist.source` · `project.part.unit`

---

## Enduring

### A year-old session still opens

t[scenario.album.restore]
A session restored from a year ago opens in Reaper with its media resolvable,
including stems renamed and consolidated since — resolution follows recorded
rename history and content addresses, not stale paths. Restoring produces a new
version and discards nothing. Reaper's own `.rpp-bak` files and peak caches never
appeared as user-facing versions, and the `2.1 Somma` and `Copy of Copy of` in
neighbouring session names were shown as labels without being mistaken for a
lineage.

_Exercises_ `files.version.restore` · `files.version.native` ·
`files.version.labels` · `files.version.unit` · `files.version.cadence`

---

### Footage moves to cheaper storage and no path changes

t[scenario.album.placement]
A new storage location is attached with no downtime and no migration, and
finished footage relocates to it. No path changed, no link broke, and the album
looks identical.

_Exercises_ `files.scale.capacity` · `files.scale.large-media` ·
`project.location.composed`

---

### The post server goes away for a week

t[scenario.album.outage]
The album still opens. Structure, parts, deliverable list and metadata stay
browsable; the post server's content is visibly unavailable; work continues on
everything else. Reconnection requires no re-adoption and no re-listing.

_Exercises_ `project.location.degraded` · `files.catalogue.offline` ·
`files.catalogue.staleness` · `files.topology.multi-server`

---

### Delete every database and lose nothing

t[scenario.album.rebuild]
Every projection database is deleted and both servers restart. The album, its
parts, its promotions, its declarations and its deliverables are all
reconstructed from the vault tree and the content store. Only derived state —
indexes, transcripts, thumbnails — has to be rebuilt, and it rebuilds. Nothing a
human wrote is gone.

_Exercises_ `storage.projection.rebuildable` · `storage.tier.derived` ·
`storage.tier.observed` · `storage.projection.write-through` ·
`vault.index.incremental`

---

## Coverage

Exercised by some stage above: every rule in `files.*`, `project.*` and
`storage.*` except those listed below.

**Not reached by this scenario, and why:**

- `project.definition.single` and `project.capability.closed` — properties of the
  codebase, not of a run. No scenario can exercise either; a second definition,
  or a free-form capability string, is what violates them.
- `project.vault.write-path` — asserted throughout rather than at any stage:
  every write above goes through the Files API by construction.
- `storage.query.reach` — a platform constraint. The scenario should be run once
  with a browser client to exercise it.
- `vault.index.parse-once`, `vault.index.tolerant`, `vault.write.granular`,
  `vault.write.atomic` — read- and write-path properties measured under load and
  fault injection, not observed in a narrative run.
- `files.scale.small-files` is exercised at 14,671 files, which is the album's
  real size and an order of magnitude below the 100k target. Running the scenario
  against the full 6.1 TB tree is a separate exercise.

**Gaps this scenario found, now closed.** Writing it exposed three contracts no
rule stated: a Files-side adoption contract (`files.adopt.*`), a definition of
what a facet actually is (`files.facet.*`), and an ignore layer for
operating-system artifacts distinct from application ones (`files.ignore.*`).
All are now specified. Archive tiering was considered and deliberately not made
a requirement: moving finished work to cheap storage is a placement policy, which
`files.scale.capacity` already covers.
