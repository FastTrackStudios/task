# 4. Vault, Assets, Resources — and a primitive that does not name its consumers

Date: 2026-09-10

## Status

Accepted. Supersedes parts of
[ADR 0003](0003-task-as-the-shared-backend.md) — decisions 2 and 3, and
the storage half of decision 4. The rest of 0003 stands.

## Context

ADR 0003 made Task the store the sibling apps share, and shipped. Using
it revealed two things wrong with it, one mechanical and one conceptual.

**The mechanical one.** 0003 filed charts, patches, samples and lighting
under `<org>/resources/`, because that is what `SourceKind::Resource`
subscriptions materialise across organisations and what
`/org/{slug}/media/{*path}` already serves. That reasoning was sound and
the consequence was not: the resources tier is plain filesystem reads and
writes — its own module doc says "the resources tier isn't the vault" —
so `upsert_chart` is a bare `std::fs::write`. A chart therefore has no
CRDT document, no collaborative editing, no wikilinks, no tags, no
search, and no presence in Task's own UI.

That is the whole of what makes a note a note. `vault-collab` gives every
*vault* file a Loro document keyed by `(vault_id, path)`, with
write-behind to disk and three-way merge against external writes. Two
people editing a chart in Keyflow should get exactly that, and under 0003
they cannot, because charts are on the wrong side of a line drawn for a
different reason.

**The conceptual one.** 0003 added `Capability::{Session, Signal,
Ignition, Keyflow}` and `CollectionKind::{Library, Setlist, Show,
Playlist}` to Task's own type system. It argued for the first explicitly:
"what `keyflow` means is precisely *Keyflow opens this*."

That argument is backwards. A primitive that enumerates its consumers is
not a primitive. Task provides a vault, a wiki, files and links; *setlist*
and *show* are things a performance application means, and the day a
sixth app appears — or Session decides a show contains rehearsals as well
as setlists — the vocabulary has to change in the layer underneath it.
Every app then waits on a release of the thing it is built on to say a
word about its own domain.

## Decision

### 1. Three tiers, distinguished by what they are for

| tier | bytes | mutable | collaborative | may reference |
|---|---|---|---|---|
| **Vault** | in the vault tree | yes | yes (CRDT) | anything |
| **Assets** | in the vault tree | yes | yes (CRDT) | anything |
| **Resources** | outside the vault tree | no | no | **nothing** |

**Vault** is the knowledge system: notes and wiki pages, the things whose
links *are* the point.

**Assets** are vault items mechanically and a separate shelf
conceptually. Any file, any directory. They are the things that will be
useful later rather than the inner workings of a knowledge base — a
chart, a patch, a session file. Being vault items is not an
implementation detail to be hidden: it is precisely what buys them
collaboration, tags, wikilinks and search without building any of it
twice.

**Resources** are imports. Sermon transcripts, scripture, a downloaded
article. They sit outside the vault tree, they do not change, and they
are **leaf nodes**: a resource may be linked *to* and may not link *out*.

That last property is the one worth stating as a rule rather than a
habit, because it is checkable. `wiki lint-tiers` already enforces a
tier boundary for wikilinks; the same shape applies here. A resource
that could reference back into the vault would make the import layer
part of the graph it was supposed to feed, and there would be no
direction left in which to say "this came from outside".

**Songs, arrangements and charts move from Resources to Assets.** They
are edited, by more than one person, which is the whole distinction.

This is wider than it first appeared, and the widening is the point.
Adding one song through the existing `task song add --chart` produces:

```text
resources/songs/opening-night/song.md
resources/songs/opening-night/arrangements/default/opening-night.kf
resources/songs/opening-night/arrangements/default/arrangement.md
```

— and leaves `vault/` holding nothing but `.fts-root.json`. So it was
never only charts that sat outside the vault: a song note, its
arrangement notes and its chart files are all there. Moving charts alone
would put a song in one tier and its own chart in another, which is
worse than either end state.

Patches, samples and lighting stay in Resources. They are binary
payloads nobody types into, and no one co-edits a WAV.

Note for whoever implements this: an `arrangements/` folder already
exists on disk, and an `arrangement` field was added to `ChartDoc`
separately. Two representations of one idea. One of them has to become
authoritative rather than both surviving quietly.

### 2. Collections carry an app-defined kind

`CollectionKind`'s named variants are removed. What remains is an
ordered collection of `NodeRef`s with a `kind: String` the *caller*
supplies, and lexorank ordering, which is all Task ever contributed.

`library`, `setlist`, `show` and `playlist` become strings that Session,
Keyflow and Ignition define, document and validate. Task neither knows
nor cares what they mean; it guarantees ordering, membership and
reference resolution.

This is a breaking change to a shipped lane and gets a migration:
existing rows carry their variant's `as_str()` value, which is already
what is persisted, so the on-disk form does not move.

### 3. Capabilities stop naming applications

`Capability::{Session, Signal, Ignition, Keyflow}` are removed from both
`project-proto` and `files-domain`. A project declares what *work* it
holds; which application opens that work is the application's business,
resolved by the node kinds and asset shapes present rather than by a
label Task maintains.

The half of `project.capability.conventions` that 0003 claimed to have
filled — deliverable kinds and UI surfaces — reverts to unmet, and
`docs/spec/unmet.md` says so. Recording an honest gap is better than a
vocabulary that has to be released to describe someone else's domain.

### 4. Task is embeddable as a library, not only reachable as a server

`TASK_EMBED` and `AppState::server_local_server` already run the whole
router in-process over a `LocalServer` with no socket. That is Task as a
library, gated behind an environment variable and documented as a CLI
convenience.

It becomes a supported surface: an external application takes Task as a
crate, opens or creates an org's data root, and drives the vault, wiki,
files and links services directly — organising its own domain in
markdown and tags, exactly as the CLI does today. The wire lanes remain
for applications that want a server; the crate is for applications that
want a store.

## Consequences

**A chart becomes an ordinary vault document.** Collaboration, tags,
wikilinks, search, sync and Task's own UI all arrive at once, because
none of them were ever chart-specific.

**Cross-organisation reach has to be re-established for Assets.** This is
the real cost, and it is the thing 0003 chose `resources/` to get.
`SourceKind::Resource` subscriptions and the `/media` range route reach
`resources/`; vault-held assets need an equivalent, and until they have
one a cross-org asset library is a regression against what 0003 shipped.
Nothing here should land before that path exists.

**Every app can name its own domain without a Task release.** The test
of decision 2 and 3 is whether Session can add a rehearsal-shaped
collection, or a seventh app can appear, without touching this
repository. It can.

**Two decisions in ADR 0003 were wrong within a week of being accepted.**
Worth recording plainly: they were wrong in the same direction, which is
the direction a store drifts when its first consumers are all in the
same building. The rule that falls out — *the primitive does not name
its consumers* — is the useful part, and it is cheap to check against
any future addition.
