# 4. Vault, Assets, Resources — and a primitive that does not name its consumers

Date: 2026-09-10

## Status

Accepted. Supersedes parts of
[ADR 0003](0003-task-as-the-shared-backend.md) — decisions 2 and 3, and
the storage half of decision 4. The rest of 0003 stands.
Amended 2026-10-21: cross-org reach for Assets (see the end).

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

### 1. Four roots, and one trait they all implement

An organisation's file hierarchy has exactly four roots:

| root | holds | cardinality |
|---|---|---|
| `vault/` | the knowledge system — notes | exactly one vault |
| `wiki/` | every wiki | many, named |
| `assets/` | every asset group | many, named |
| `projects/` | every project and sub-project | many, nested |

All four implement one trait: a **shelf** — a named, file-backed root
that registers for sync and per-file CRDT, and that may be published,
subscribed to and resolved into.

That trait is the whole of decision 1. Everything below is a consequence
of it rather than a separate mechanism.

| shelf | mutable | collaborative | subscribable | may reference |
|---|---|---|---|---|
| Vault | yes | yes | **no** — a vault is shared by link, not subscription | anything |
| Wiki | yes | yes | yes | anything |
| Assets | yes | yes | yes | anything |
| Projects | yes | yes | yes | anything |
| *Resources* | no | no | yes | **nothing** |

Resources are an asset group that happens to be immutable and
leaf-only: imports, held whole, never authored here. They are listed
apart because those two properties are worth naming, not because they
are a fifth root.

**Collaboration follows registration, not location.** This is the fact
the trait is built on, and it was learned the expensive way — see the
note at the end of this section.

### 1a. Subscriptions are granular

A subscription names a shelf *or part of one*.

An asset group "Live Tracks" may hold collections — "Worship Tracks",
"Pop Tracks" — and an organisation subscribing to it should be able to
take the worship material and not the rest. Whole-shelf subscription is
the degenerate case, not the only one.

The same selection governs what a *machine* keeps locally. Subscribing
says what this organisation may see; syncing says what this laptop
bothers to hold. A studio machine takes the multitracks; a phone takes
the charts and leaves forty gigabytes of stems on the server. Both are
the same question — *which part of this shelf?* — asked at different
scopes, and they should share one vocabulary rather than growing two.

This is why the four roots matter beyond tidiness. Large content lives
on shelves the vault merely *references*, so a vault stays small and
fully synced while the material it points at is fetched on demand. A
reference resolving to something not resident locally is an ordinary
state, not an error.

The mechanism already exists at the Files layer: `files_domain::Facet`
and `FacetMap`, with `files.sync.selective` describing atomic facets
that bring their dependencies. Granular subscription should be that
primitive addressed from the subscription lane — not a second selection
system with its own rules.

**Vault** is the knowledge system: notes and wiki pages, the things whose
links *are* the point.

**Assets** are a tier of their own — `<org>/assets/<kind>/`, beside
`vault/`, `wiki/` and `resources/` — holding the things that will be
useful later rather than the inner workings of a knowledge base. Any
file, any directory, at any size. A chart, a song, a session file.
There are several kinds, and each kind is its own shelf.

An earlier draft of this ADR made Assets a subtree of the vault
(`<vault>/Assets/<Kind>/`) to obtain collaboration. That was solving a
problem that did not exist. The `wiki/` tier is not inside the vault
either, and its pages are already collaborative — `wiki_editor_e2e`
states it plainly: the wiki editor is "`VaultSync` files, per-file CRDT
collab", and `two_collab_sessions_converge_on_a_wiki_page` proves for a
wiki page what `vault_collab_e2e` proves for the vault. Per-file CRDT
follows the wiring, not the directory. Assets get it the same way the
wiki tier does.

**What Wiki and Assets share is a trait, not a hierarchy.** Both are
*subscribable sources*: a named shelf of files an organisation may
publish, another may subscribe to, and a reference may resolve into.
That is the abstraction, and both tiers implement it — rather than one
being a special case of the other, which would make every future shelf
either a wiki-shaped compromise or a second copy of the same machinery.

So `<org>/assets/songs/` is a song library another organisation can
subscribe to, exactly as it subscribes to a wiki today, and gets the
visibility rules, materialisation and reference resolution that already
exist for wikis. Nothing about subscription is asset-specific.

The tier is also where the size difference lives. A wiki carries
`media/` for images and attachments; an Asset shelf is where large
content belongs, over the Files layer that is already mounted on these
roots. That is the only mechanical difference between the two, and it
is a difference of degree rather than of kind.

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

## Amendment — 2026-10-21: cross-org reach for Assets is closed

The Consequences above say cross-organisation reach "has to be
re-established for Assets", and that until it is, a cross-org asset
library is a regression against what 0003 shipped. That path now exists.
Another org's song library is subscribed to, kept, and referenced on the
same terms as a wiki:

- **Subscribe.** `SourceKind::Assets` names one shelf, `<org>/assets/<kind>/`,
  by its kind (`acme.test/songs`). Over MCP, `subscribe_wiki` takes
  `kind: "assets"` (`docs/mcp-tools.md`). `resource: true` stays an alias
  for `kind: "resource"`. Tests: `apps/server/src/mcp.rs` —
  `subscribe_kind_defaults_to_wiki`, `subscribe_kind_takes_every_advertised_name`,
  `the_resource_flag_is_an_alias_for_the_resource_kind`,
  `the_resource_flag_contradicting_kind_is_refused`,
  `an_unknown_kind_is_refused_naming_the_valid_ones`,
  `the_qualified_id_shape_follows_the_kind`; and the MCP lane end to end in
  `apps/server/tests/it/mcp_wiki_e2e.rs`.
- **Admission.** A shelf is private to its org unless it is unlisted or
  public, or the owner granted the subscribing org. A refusal reads the same
  whether or not the shelf exists. Tests:
  `features/wiki/wiki-live/src/subscriptions_backend.rs` —
  `a_private_shelf_or_project_is_refused_to_an_outsider_without_saying_it_exists`,
  `an_unlisted_shelf_is_admitted_by_reference_and_listed_only_when_public`,
  `a_grant_admits_on_the_same_disk_and_a_revoke_binds_on_the_next_refresh`,
  `a_grant_secret_admits_on_the_remote_path_and_a_read_secret_alone_does_not`.
- **Keep.** A refresh materialises the shelf into
  `subscribed/<domain>/<kind>/`, and the copy outlives a revoked grant
  (the same `a_grant_admits_on_the_same_disk…` test).
- **Reference.** A reference carries no kind, so the held subscription
  supplies one. `[[acme.test/songs::hallelujah]]` resolves to the song on
  the shelf for an org holding `acme.test/songs` as `assets`, and to a wiki
  named `songs` when nothing of that name is held, which is what a reference
  meant before shelves. Admission still decides: a held subscription whose
  grant was revoked resolves nowhere, and it does not fall back to the wiki.
  Tests, same file:
  `a_held_asset_subscription_resolves_a_reference_to_the_shelf_page`,
  `with_nothing_held_a_reference_defaults_to_the_wiki`,
  `the_held_kind_picks_between_a_wiki_and_a_shelf_of_one_name`,
  `a_declined_asset_subscription_does_not_steer_resolution`,
  `a_revoked_grant_stops_resolution_into_the_shelf`.

Three edges are still open, and this amendment names them so they are not
mistaken for closed:

1. **One subscriber cannot hold a wiki and a shelf of the same name from
   one domain.** Subscription identity is `domain/slug` without the kind,
   and both kinds would materialise into the same `subscribed/<domain>/<slug>/`
   directory. The second subscription is refused as a duplicate, so nothing
   is overwritten, but the pair cannot be held. Test:
   `a_wiki_and_a_shelf_of_one_name_collide_in_one_subscriber`. Fixing it
   means identity that includes the kind, plus a copy-directory migration.
2. **`ResolvedReference` does not say which tier it resolved into.** A
   client following the answer gets `org`, `wiki` (the source slug) and
   `path`, and has to know from elsewhere that `songs` is a shelf before it
   can open it. Adding the kind to the answer is a wire change to the
   `wiki/subscriptions` service.
3. **`resolve_reference` reads the publisher's tree only when it is on the
   same data root.** For a source on another server it answers `None`, even
   when it is held, because it does not read the subscriber's copy in
   `subscribed/<domain>/<slug>/`. The `song:` node resolver
   (`node_homes::LocalHomes`, ADR 0003) already answers from that copy. A
   `::` reference across servers needs the same fallback.

The link form for another org's songs, in notes, setlists and the gig recipe
(`docs/app-integration.md`), is `[[<domain>/songs::<slug>]]`.
