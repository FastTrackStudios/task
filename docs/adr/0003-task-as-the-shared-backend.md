# 3. Task is the backend the sibling apps share

Date: 2026-09-05

## Status

Accepted.

## Context

Five apps sit on one account: Task, Session, Signal, Ignition and Keyflow.
`fts-auth` already makes that one account real — each app is a registered
OIDC client, and `central_auth` resolves any of their tokens to the same
principal. What none of them share is a place to keep anything.

Each is about to grow the same need, phrased four ways:

- **Session** builds song libraries for an organisation, and setlists that
  draw on more than one of them — a guest musician's own library beside the
  church's.
- **Signal** keeps patches and sample libraries, and wants a patch attached
  to a song rather than filed beside it.
- **Ignition** keeps lighting for a song, a setlist and a show.
- **Keyflow** wants a person's charts to survive the browser tab, and a
  Session setlist to reach the chart for each of its songs.

Four apps, one sentence: *a library of typed things, referenced into a
performance, shared across organisations.* Building that four times gives
four vocabularies, four sync stories, four auth stories, and no path from a
setlist to the chart it needs.

Task already answers most of it, in pieces written for exactly this:

- `project.setlist.source` — "a setlist is assembled by reference rather
  than by copying. A setlist references songs as parts, promoted or not …
  and a song may appear in any number of setlists."
- `collection_proto::CollectionKind` — `Library`, `Setlist`, `Show`,
  `Playlist`, described in its own module docs as "the *same* thing: an
  ordered collection of references to nodes that already live elsewhere in
  the graph", ordered by lexorank.
- `project.form.components` — "a song optionally carries a chart and zero or
  more sessions; a session is a component, not a project."
- `project.capability.multiple` — capabilities are a set, and each one
  "declares … its deliverable kinds, and the UI surfaces the project
  offers."
- Subscriptions — the one cross-organisation mechanism that works today,
  addressing a source as `domain/slug` (ADR 0002) with per-source
  visibility re-checked on every refresh.

Three things are missing, and they are the whole of this decision.

## Decision

### 1. A node reference may name another organisation

`links_proto::NodeRef` becomes `{domain, kind, id, anchor}`, where an empty
`domain` means the reader's own organisation. The token grammar gains one
optional leading segment, mirroring ADR 0002's:

```text
fasttrackstudio.app/song:keep-on-finding-more#t:1:30
└──── domain ─────┘ └kind┘└────── id ───────┘└anchor┘
```

Two forms, and they mean different things:

- **Qualified** — `domain/kind:id`. Names exactly one node in the
  federation. Two organisations cannot collide on a domain, so this text
  means the same song in every vault that holds it.
- **Local** — `kind:id`. The reader's own organisation, and never anything
  subscribed.

There is deliberately no *short* form. ADR 0002 has one because a reader
holds a handful of wikis and `theory::Ionian` is worth the ambiguity; a
reader holds thousands of songs, and a bare `song:hosanna` that silently
means a different recording in a different vault is the failure this whole
decision exists to avoid.

Parsing stays total, as `Reference::parse` is: anything that does not fit is
read as a local reference rather than refused, because the alternative is a
setlist that will not open because somebody typed a colon.

**Resolution is the reader's, and grants nothing.** A qualified reference
resolves only if the reader already has access by a mechanism that exists
today: a membership row in that organisation, or a subscription to a source
that publishes the node. A reference is not a credential, it is an address —
`wiki.subscribe.resolution` already says this for pages and it is true here
for the same reason. An unresolvable reference is reported as unresolved and
rendered as such. It is never an error, and never guessed at.

### 2. Capabilities name the four apps

`project_proto::Capability` gains `Session`, `Signal`, `Ignition` and
`Keyflow` beside `MusicProduction` and `VideoProduction`. A project holding
all six is ordinary: a concert film that is recorded, cut, performed from
charts, lit, and played back through a rig.

This mixes two naming schemes, and that is the deliberate part. The existing
two name *work*; the new four name *apps*. The alternative — `charting`,
`lighting-design`, `tone-design`, `live-performance` — was considered and
rejected. `project.capability.conventions` defines a capability as declaring,
among other things, "the UI surfaces the project offers", and for these four
the surface **is** the app: what `keyflow` means is precisely "Keyflow opens
this". Naming the work instead would leave every one of them mapping to
exactly one app anyway, with a second vocabulary to keep in step.

The vocabulary stays closed and small (`project.capability.closed`). Six
members remain interpretable by a sync client, a placement policy and a UI,
which is the test that rule sets.

This also completes the half of `project.capability.conventions` that
`docs/spec/unmet.md` records as having "no home at all": deliverable kinds
and UI surfaces get one, because an app is what provides them.

### 3. A library is a collection; an asset is a node

Nothing new is built for libraries. `CollectionKind::Library` over
`NodeRef`s is already the model, and with qualified references it is already
cross-organisation: a setlist in one organisation holding
`guest.example/song:hosanna` beside a local `song:doxology` needs no new
type, no new lane and no new store.

`NodeKind` gains the asset kinds the four apps keep, each with a stated home
under `<org>/resources/`, as `Song` and `Sermon` already have:

| kind | app | home | anchor |
|---|---|---|---|
| `Chart` | Keyflow | `resources/charts/<slug>.kf` | a section (`chorus`) |
| `Patch` | Signal | `resources/patches/<slug>/` | — |
| `Sample` | Signal | `resources/samples/<slug>/` | a region (`t:0-2:400`) |
| `Lighting` | Ignition | `resources/lighting/<slug>/` | a cue (`cue:12`) |

`resources/` is chosen over a new tier for one reason: it is already what
`SourceKind::Resource` subscriptions materialise across organisations, and
already what `/org/{slug}/media/{*path}` serves with range requests. Putting
the assets there makes a cross-organisation patch library work through
machinery that is built, rather than through machinery that would have to be.

A sample *library* is not a fifth kind — it is a `Collection` of kind
`Library` over `Sample` nodes, which is the point of having the collection.

### 4. The apps are clients, not backends

An app reaches Task the way Task's own web client does, and gains nothing
extra by being first-party:

```text
app → fts-auth (OIDC, its own client id) → access token
    → task.fasttrackstudio.app → central_auth resolves the principal
    → membership row fences the organisation
    → the permit table gates the method
```

No app gets a private lane, a shared secret, or a server of its own. Keyflow
saving a chart is `collection.add_item` and a write under `resources/charts/`,
through the same permits as everything else, and an app with no membership
row in an organisation reads nothing from it — `memberships::role_for`
returning `None` is a refusal, never a fallback.

Server state for a new asset kind is added the way every lane is: a
`-proto` crate, a backend, an entry in `permits::mounts()`, and a
`plugin`-guarded branch in `org_layer_router`. Screens, note widgets, fences
and link claims need none of that — `PluginApp` already carries them.

## Consequences

**A setlist can finally span organisations.** The thing the four apps each
needed separately is one qualified reference in one existing collection type.

**The `Song` node becomes the join.** A song already carries a chart and
sessions as components; it now also carries patches and lighting by
reference. Session asks for the setlist, Keyflow renders the chart of each
song, Signal loads the patch attached to it, Ignition runs its cues, and none
of the four has to know the others exist.

**Domain becomes part of node identity, and existing tokens keep working.**
An empty domain is local, so every `song:hosanna` written to date parses
unchanged. The `links.jsonl` and `collections.jsonl` stores gain an optional
field, not a migration.

**Cross-organisation reads stay as tight as they are.** Nothing here widens
access: a reference resolves through a membership or a subscription, both of
which already refuse by default and are re-checked on refresh. What is new is
the ability to *address* something in another organisation, which is not the
same as the ability to read it, and the distinction is the reason this is
safe to ship before remote upstreams exist.

**The remote upstream becomes the next constraint.** `LocalOrgs` resolves
sources published by other organisations on the same data root, which is what
the demo and the suite exercise. A qualified reference to an organisation on
*another server* will parse, and will not resolve, until `trait Upstream`
grows the remote implementation its own docs describe as "the same
materialize call against a `VaultSyncClient`". That is the honest boundary of
this decision, and it is the next piece of work rather than a caveat hidden
in it.

**Ignition and Signal have no server-side home yet.** Neither appears
anywhere in the tree today. The node kinds and capabilities here are the
sockets they plug into; the plugins themselves are still to be written.
