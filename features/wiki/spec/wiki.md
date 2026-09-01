# Wiki Spec

The testable form of *many wikis per org*: what a wiki is as distinct from a
vault, what can be subscribed to, how the pages of separate wikis form one web,
how someone without the Editor role proposes an **Edit Request**, and how a wiki
mirrored from a git repository behaves like every other wiki.

A wiki is a vault. Same primitives, same tree, same page model — the difference
is reach and nothing else: a wiki is publishable, so others may subscribe to it,
link into it, and ask to change it. A vault is not.

A subscription is the live form of a reference. Citing a source, or copying it
into a vault, takes a snapshot the citer then owns; subscribing keeps a channel
open to a body that goes on being added to and corrected. Both are legitimate —
these rules govern the second.

Two things can be subscribed to. A **Wiki** is authored knowledge, and an Edit
Request is how an outsider changes it. A **Resource** is an external work pulled
in to be referenced — a book, a video, a scripture text — never authored here
and never edit-requestable. The Bible Resource is the text; the Bible Wiki is
the curated collection about it; they are different things and you subscribe to
both.

Reference format is [`../../../docs/adr/0002-wiki-reference-format.md`](../../../docs/adr/0002-wiki-reference-format.md).
The disk layout is `wiki-proto`'s `paths` module; the vault's own read and write
path is [`../../vault/spec/vault.md`](../../vault/spec/vault.md). Vocabulary is
[`../../../CONTEXT.md`](../../../CONTEXT.md).

Reference a requirement by id — `wiki.many.set` — in issues, commits and code
(`t[impl wiki.many.set]`).

Current behaviour these rules exist to replace: the server mounts exactly one
wiki per org, hard-coded as `WikiBackend::single("default", …)` over
`<org>/wiki/Knowledge/`. The service traits already take a `wiki_id` and
`federation::PeerWiki` already describes a peer, but nothing creates a second
wiki, nothing subscribes to one, and no page can be changed by an account that
cannot already write the org's vault.

---

## Multiplicity

### An org holds a set of wikis

t[wiki.many.set]
An org holds any number of wikis. Each has its own root, schema, purpose, index,
log, review queue, findings, graph and peer set, and none is privileged over
another. Creating, renaming or deleting one wiki leaves every other wiki
byte-identical. An org with one wiki is a set of size one, not a special case,
and an org with none is legal.

---

### A wiki's identity outlives its title

t[wiki.many.identity]
A wiki has a stable slug, unique within its org, that is not its display title.
The slug is what references, subscriptions and history refer to; retitling a
wiki breaks no reference and drops no subscriber. Two wikis in one org never
share a slug, and a slug, once used, is never reassigned to a different wiki.

---

### State is per-wiki

t[wiki.many.isolation]
Every operation names the wiki it acts on, and acts on no other. An index, a
lint pass, a graph, an ingest queue or a review queue computed for one wiki
reports that wiki's pages only. The sole exceptions are relations this spec
declares to cross a boundary — cross-wiki references (`wiki.ref.format`) and
their backlinks (`wiki.link.mutual`).

---

### Every wiki is addressable and enumerable

t[wiki.many.addressable]
Each wiki is reachable at `<org>/<wiki-slug>` across the whole service surface —
locally, over the network, and from the CLI — with no method that only works on
a default wiki. Listing an org returns the wikis the caller may see, each with
its title, purpose, visibility and page count, and omits the ones they may not.

---

## Vault and wiki

### A wiki is a vault that can be published

t[wiki.boundary.role]
A wiki is built on the vault's primitives and differs in one respect: it is
publishable, and so it can be subscribed to, referenced from other orgs' pages,
and changed by people who do not work for its owner. Everything else — the tree,
the page model, indexing, history, sync — is the same machinery, and a
capability added to one is available to the other unless this spec says
otherwise.

---

### A vault is never subscribable

t[wiki.boundary.no-subscribe]
A vault that has not been promoted cannot be subscribed to, by any route.
Sharing a vault or a note from one goes through a share link, which grants
reading a named thing and never makes that thing resolvable inside someone
else's writing. Attempting to subscribe to a vault is refused with that
distinction stated, not silently coerced into a link.

---

### A vault can be promoted whole

t[wiki.promote.vault]
A vault can be promoted into a wiki. Promotion changes what the tree *is* to
the rest of the system — from that moment it may be subscribed to — and changes
nothing about its contents, paths or history. Because promotion is what makes
private writing public, it states what will become visible and takes an explicit
confirmation; it is never a side effect of another action.

---

### A note can be contributed singly

t[wiki.promote.page]
A single vault note can be contributed to an existing wiki without promoting the
vault it came from. The page arrives with its content intact and its origin
recorded; the vault keeps a reference to it, and vault references that pointed
at the note keep resolving. Contributing is per-note and repeatable — a second
note into the same wiki disturbs nothing already there — and the contributed
page is thereafter the wiki's to curate, not a mirror the vault re-overwrites.

---

## Subscription

These rules say **source** for whichever of a Wiki or a Resource is in hand.

### A subscription makes another source's content resolvable

t[wiki.subscribe.reference]
Subscribing to a source makes its content behave, in the subscriber's own
writing, as if it were local: it resolves as references, appears in search and
in the graph, and embeds. A subscription is held by a vault or by a wiki, names
the source by its qualified id, and takes effect without editing a single
existing page.

---

### What may be changed follows what kind of source it is

t[wiki.subscribe.editability]
A Wiki's local copy is editable and its changes can be sent back
(`wiki.subscribe.push`). A Resource's is not: nothing is ever merged into a
Resource, and a subscriber is told so up front rather than discovering it when a
push is refused. A subscriber may additionally lock an editable copy for
themselves, to keep from changing what they meant only to read; they can never
do the reverse.

---

### A local copy is your own state, and editable

t[wiki.subscribe.working-copy]
The local copy of a subscribed wiki is not a read-only mirror: it is state the
subscriber owns and may edit freely, in the app or in any editor pointed at the
folder. Editing needs no permission on the upstream wiki and no ceremony
beforehand — the first keystroke opens a working change on top of the upstream
state, the way a working copy sits on top of a repository. What a subscription
withholds is not writing; it is writing *upstream*.

---

### There is no commit step

t[wiki.subscribe.no-ceremony]
Editing a local copy requires no commit, no staging, no stash and no branch.
Work is snapshotted as it happens, so a save is already a version and there is
never a state that must be tidied up before doing something else. A subscriber
can move between upstream's version and their own, refresh, walk back through
history and return to where they were working, without losing an edit and
without being asked to name one. Nothing a person does in the folder can leave
the copy in a state only a version-control expert can get out of.

---

### Local edits never reach upstream by themselves

t[wiki.subscribe.local-authority]
No local edit changes the source it was made against. A subscriber's changes
stay theirs until they are pushed, and are always distinguishable from what
upstream said: a page is showable as upstream's version, as the local version,
and as the difference. Nothing about holding local changes degrades reading —
the copy goes on refreshing, and a subscriber who edits is not thereby
un-subscribed or frozen at a version.

---

### A subscribed source is present locally

t[wiki.subscribe.local-copy]
A subscribed source is materialised locally, so its references resolve with the
network down and rendering a page fetches nothing. The copy records which
upstream state it reflects and how stale that is, and refreshes without blocking
reads. Unsubscribing removes the copy and leaves the subscriber's own prose
untouched: the references that pointed into it become unresolved, and no text is
rewritten. Unsubscribing from a copy carrying unpushed local changes says so and
takes an answer, rather than discarding work to tidy up.

The exception is a Resource whose licence forbids holding its text
(`wiki.resource.rights`): its addressing is held locally and always resolves,
while the text itself arrives per passage.

---

### A refresh rebases local work rather than overwriting it

t[wiki.subscribe.refresh]
Refreshing a subscribed copy replays the subscriber's changes onto the new
upstream state. Local work is never overwritten by an upstream update, and
upstream is never quietly reverted by a stale local copy. Where the two touched
the same lines the conflict is surfaced against both versions and left for a
person; it is never resolved by recency, and a copy left conflicted stays
readable while it waits.

---

### Pushing local work back opens an Edit Request

t[wiki.subscribe.push]
Pushing is the subscriber's decision and happens when they say so. Nothing
auto-pushes, no amount of accumulated local work starts a push on its own, and
work may sit local indefinitely without nagging or expiring. A subscriber
chooses what goes up, too: pushing some pages while others stay local is
ordinary, not a workaround.

What arrives upstream is an Edit Request (`wiki.edit.request`) against the
wiki's home, whether the change was typed in the app or made by an outside
editor in the mounted folder. Accepted, it lands upstream and the subscriber's
next refresh no longer carries it as a local change, because it is now what
upstream says. Rejected, the work stays in the local copy: a rejection upstream
is not a deletion here.

---

### Subscription is not transitive

t[wiki.subscribe.transitive]
Subscribing to a source does not subscribe you to what that source subscribes
to, the core set (`wiki.core.default`) excepted. Nothing enters your
subscription set because a source you subscribe to happens to hold it; a source
you have not taken on is one you do not have, however many of your subscriptions
depend on it.

---

### Resolution reads the reader's own set

t[wiki.subscribe.resolution]
A reference resolves when its target is in the *reader's* subscription set,
whoever wrote the reference and wherever the page carrying it lives. Subscribe
to Music Theory and to Audio Production, and Audio Production's references into
Music Theory resolve for you — you already hold what they name, and there is
nothing left to grant. What non-transitivity withholds is acquisition, never
rendering: two readers of the same page legitimately see different references
resolve, and neither is an error.

A reference whose target is genuinely outside the reader's set renders as
unresolved, names the source it wanted, and offers subscribing to it. It never
resolves through someone else's subscription, and never reports a missing page
when the truth is an unknown source.

---

### What a wiki subscribes to is visible and adoptable

t[wiki.subscribe.inherit]
A subscriber can see the subscriptions a wiki it subscribes to holds, and take
any of them on with one action. An adopted subscription is thereafter the
subscriber's own — held by them, listed among theirs, and unaffected by the
upstream wiki later dropping it. Declining leaves the offer standing and changes
nothing else: a declined source you happen to hold already still resolves, by
`wiki.subscribe.resolution`, because the reason it resolves was never the
upstream wiki's subscription.

---

### A remote source subscribes like a local one

t[wiki.subscribe.federated]
A source on another server subscribes through the same surface as one in the
same org: same id shape, same staleness reporting, same resolution. Whether a
subscribed source is local, on a peer, or on the central deployment changes
latency and nothing else a reader or writer can observe.

---

## References

### A reference is readable, qualified, and stamped

t[wiki.ref.format]
A reference into another source carries the publishing org's federation domain,
the source's slug, and the target — `[[fasttrackstudio.app/music-theory::Ionian]]`
— plus the moment the reference was made. It is readable: the slug appears
literally, never as an opaque id, because the file has to make sense in Obsidian
and to an agent reading it raw. It is unique by construction: two orgs cannot
collide on a domain, so the same reference text means the same page in every
vault that holds it. Unqualified `[[Page]]` resolves locally only and never
silently reaches into a subscribed source.

---

### Authoring never requires typing a qualified reference

t[wiki.ref.picker]
Opening a reference offers everything the writer subscribes to — wikis,
Resources, pages, blocks — and inserting from that list writes the qualified
form. The editor renders it as the short title, so writing and reading stay
legible while the file stays unambiguous. Nobody has to know a domain to cite a
page.

---

### A section is referenced by anchor, not by heading

t[wiki.ref.block]
A reference to part of a page names a block anchor (`#^short-id`), not a
heading. Retitling the section, moving it within the page, or rewriting the
text around it does not break the reference. The anchor form is the one an
outside markdown editor also resolves, so a reference stays live in a mounted
folder.

---

### The stamp makes staleness derivable

t[wiki.ref.stamp]
A reference always resolves to the *current* state of its target — the stamp
never pins, and a reader is never quietly shown an old version. The stamp exists
so drift is visible: a reader can be told the target has changed since the
reference was made, and can recover what it said at that moment from history
when something looks wrong. Both readings come from data already held; neither
requires the author to have done anything extra.

---

### References resolve across a home that moved

t[wiki.ref.redirect]
A source keeps its original qualified reference for life, including after its
publishing org is gone and another org has adopted it (`wiki.life.adopt`). The
org registry holds the redirect, and resolution follows it without a byte of
anyone's prose changing. The domain in a reference is therefore a name, not a
live address. A command rewrites ids across a vault for anyone who wants the
tidier name; it is invoked deliberately and never runs on its own.

---

## Linking

### Wikis reference each other and stay separate

t[wiki.link.mutual]
A reference from one wiki into another produces a backlink visible from the
target, subject to what the target may see of the source. Subscribed sources
present as one navigable web while every page keeps exactly one owning wiki:
nothing about being referenced from elsewhere moves a page, changes its history,
or gives the referencing side a say in its content.

---

### A reference survives a rename upstream

t[wiki.link.stable]
A cross-source reference resolves by page identity, not by path. A page renamed
or refiled in the wiki that owns it keeps resolving from every subscriber
without those subscribers editing anything. A page genuinely removed upstream
reports as removed, with its last known title, rather than as a typo.

---

### History repairs references

t[wiki.link.repair]
Because every page's full version history is kept, a rename or a move is a
recorded event rather than an inference. References that named the old title are
updated from that record — across wikis and across subscribers, without guessing
from similarity and without a scan of every page. A repair is itself a version:
visible in history, attributable, and reversible. A reference the record cannot
account for is left alone and reported rather than rewritten on a hunch.

---

### Borrowed knowledge is visibly borrowed

t[wiki.link.provenance]
A resolved cross-source reference shows which source answered it, on the
reference and on the page it opens. A reader can always tell their own writing
from a subscribed source's, and a page pulled in from a subscription is never
presented as locally authored.

---

## Edit Requests

The unit of contribution is an **Edit Request**: one proposed change to a wiki
from someone without the Editor role on it. Requests are worked through the
**Edit Tracker**, which is a view of the issue system rather than a tracker of
its own.

### Anyone who can read can open an Edit Request

t[wiki.edit.request]
An account that may read a wiki may open an Edit Request against it without
holding the Editor role. A request carries the changed pages as a change — the
edited content itself, against a named version — not a message describing one,
and opening one never mutates the wiki.

---

### An Edit Request is an issue

t[wiki.edit.tracked]
An Edit Request *is* an issue on the tracker of the wiki's owning org — one row,
viewed through the Edit Tracker — carrying its proposer, its target wiki and its
change. Comments, status, assignment, labels, cycles and search work on it
exactly as on any other issue. Closing the issue and resolving the request are
the same event, so the two can never report different states, and a request
closed from the issue surface is closed in the wiki.

---

### Editor is a role on one wiki

t[wiki.edit.editor]
Editor is held on a particular wiki, not on an org: it says who may accept
changes into that wiki and nothing about any other. It is granted by that wiki's
owner or by an admin of the owning org, and it is visible — a contributor can
see who will review their request before they open it. Org membership alone
never confers it.

---

### An Editor's own change is auto-approved, not exempt

t[wiki.edit.auto-approve]
An Editor's change goes through the same lane as everyone else's and is approved
automatically within it. It is recorded as a change with an author and a tracker
row, exactly as a reviewed one is, so "every change to this wiki and who made
it" stays a single query. An Editor may also open their own change for review
instead, and that is the same mechanism with the auto-approval declined.

---

### A reviewer sees a diff and decides

t[wiki.edit.reviewable]
An Edit Request is reviewed as a diff against the current page. It can be
accepted, rejected, or returned for changes. Accepting lands it as a version in
the wiki's history attributed to the proposer. Rejecting leaves the wiki
byte-identical to before the request was opened, and neither outcome loses the
request's text.

---

### A stale request still applies

t[wiki.edit.rebase]
A request made against an older version of a page still applies when it does not
conflict with what landed since. When it does conflict, the conflict is shown to
both the reviewer and the proposer and left for a person to resolve; it is never
silently taken, silently dropped, or resolved by recency.

---

### Requests travel; acceptance does not

t[wiki.edit.home]
Edit Requests propagate peer to peer, so a request reaches Editors wherever they
are and a contributor need not be able to reach the home server to open one.
Acceptance happens only at the wiki's home. A peer relays requests and receives
results; it never merges. So "accepted" has exactly one meaning and two Editors
on two peers cannot accept contradictory changes to the same page.

---

### A request under review is claimed

t[wiki.edit.claim]
An Editor claims a request before reviewing it, and a claim is visible to other
Editors, so two people do not review the same request in parallel. A claim
expires rather than sticking to someone who walked away, and expiry returns the
request to the queue without losing anything on it.

---

### Each wiki declares who may propose

t[wiki.edit.gate]
A wiki declares who may open an Edit Request against it, independently of who
holds Editor. No request becomes a write without an acceptance by an Editor.
Requests from accounts the wiki does not vouch for are held, never published,
and a wiki that has closed requests says so instead of accepting ones it will
never look at.

---

## Resources

A **Resource** is an external work pulled in to be referenced: a book, a video,
a paper, a scripture text. It is read, cited and annotated, never authored here.

### A Resource is subscribed to, not imported

t[wiki.resource.subscribe]
A Resource is subscribable on the same terms as a wiki: an id, a local presence,
staleness, and reference resolution. Subscribing to one is distinct from citing
it or copying passages into a vault — a citation is a snapshot the citer then
owns, while a subscription keeps receiving what is added and corrected upstream,
and the two are never conflated in either direction.

---

### A Resource is not a wiki

t[wiki.resource.not-a-wiki]
A Resource has no schema, purpose, index, curator, review queue or Edit lane,
and nothing requires it to grow one to be subscribable. Every rule governing
curation applies to wikis only; every rule governing subscription, referencing
and local presence applies to both. A surface listing what a vault subscribes to
shows wikis and Resources together and says which is which.

---

### A Resource keeps its own addressing

t[wiki.resource.addressing]
A Resource that has a canonical way to name its parts keeps it, and references
resolve through that naming rather than through page titles. A reference into
scripture resolves by verse, and goes on resolving across translations, editions
and re-imports, because the address is the verse and not a file path. A Resource
that can offer no such address can be read but not annotated.

---

### A Resource carries no annotations

t[wiki.resource.no-annotations]
Nothing is written into a Resource — not by a subscriber, not by an Edit
Request, not by the org that publishes it. Everything anyone says about it lives
in a wiki or a vault, anchored to the Resource's canonical address. The Bible
Resource holds the text; the Bible Wiki holds the history and context; a
reader's own devotional journal stays in their vault. All three anchor to the
same verse.

---

### Annotations gather against the Resource

t[wiki.resource.layers]
Reading a Resource shows the annotations anchored to what is being read, drawn
from every source the reader holds — their own vault, the wikis they subscribe
to — each labelled with where it came from (`wiki.link.provenance`). Sources can
be switched off for reading without unsubscribing, so a reader turns
perspectives on and off; the choice persists for that reader and changes nothing
for anyone else.

---

### A Resource declares its rights

t[wiki.resource.rights]
Every Resource carries its licence and how its content reaches a subscriber.
Content that may be redistributed is held locally like any other subscription.
Content that may not — a licensed translation fetched per passage under the
reader's own key — is never persisted as redistributable files, and the rule
holds on every device the subscription reaches. Addressing is always held
locally, so references resolve offline even when the text they name does not
render.

Resources are published only by this platform's own org for now; third-party
publication, and the machinery for asserting a right to publish, is out of
scope. The declared licence is not: it drives behaviour regardless of who
published, so it ships with the first Resource.

---

## Core subscriptions

### Some subscriptions are on by default

t[wiki.core.default]
A deployment declares a **core** set of wikis and Resources — scripture among
them. Every vault and every wiki carries the core set from the moment it exists,
with nobody opting in: a note written in a brand-new vault can reference a core
Resource in its first line. Core membership is a property of the source, not a
copy of it — a core Resource is the same Resource everyone else subscribes to,
held once and resolved from everywhere.

---

### A core subscription can be declined per vault and per wiki

t[wiki.core.optional]
Core means on by default, not mandatory. Each vault and each wiki may turn one
off for itself, and doing so affects only that vault or wiki — never another,
and never the source. A decline stays declined across restarts, resubscription
sweeps and additions to the core set, and is re-offered rather than
re-subscribed.

---

### The core set reaches what already exists

t[wiki.core.retroactive]
Adding a source to the core set subscribes every existing vault and wiki that
has not declined it, not only the ones created afterwards. Removing one from the
core set stops it being handed to new vaults and leaves those already using it
subscribed, now on their own account — no reference breaks because a deployment
changed its defaults.

---

## Access and discovery

### Three visibilities, three behaviours

t[wiki.access.visibility]
A wiki is public, unlisted, or private, set per wiki and independently of every
other wiki in the org. Public appears in discovery and anyone may subscribe.
Unlisted appears to nobody but may be subscribed to by anyone holding its
reference. Private is refused: not listed, and a subscription attempt from
outside the owning org fails rather than succeeding quietly. The difference
between unlisted and private is a refusal, and the two are never conflated.

Changing visibility takes effect on what is already published: narrowing it
stops resolving for those who lost access, and does so without deleting the
wiki.

---

### Discovery is a peer, not a gatekeeper

t[wiki.access.directory]
Wikis open to subscription can be searched. The central deployment carries the
directory, and it is one peer among others rather than a component the system
requires: with it unreachable, everything already subscribed goes on resolving,
subscribing by reference goes on working, and peers go on serving what they
hold. What degrades is finding sources you did not already know about.

---

## Lifecycle

### A departing org can hand a wiki on

t[wiki.life.adopt]
An org shutting a wiki down can offer it for adoption rather than deleting it,
and another org or account may claim it and become its home. The wiki keeps its
original qualified reference (`wiki.ref.redirect`), so every reference in every
subscriber's vault goes on resolving and no prose is rewritten. A wiki nobody
adopts is not destroyed: subscribers keep their local copies and go on reading
them.

---

### A handover is authorised, not assumed

t[wiki.life.handover]
A redirect that repoints an existing reference is the highest-value thing to
forge in this system, so it is authorised: the departing org signs the handover
while it still can, and any peer can verify the signature without trusting the
registry. An org that vanished without signing can still be succeeded, but an
unsigned redirect is an offer each subscriber accepts rather than a fact that
takes effect silently. No subscriber's references are repointed by a claim they
never saw.

---

### An orphan stays readable

t[wiki.life.orphan]
A subscribed source whose home is unreachable — gone, offline, or deleted — goes
on resolving from the local copy, marked as orphaned with when it was last
heard from. What stops working is pushing, not reading. Deletion upstream never
removes content from a subscriber's disk: the possession `wiki.local.mount`
promises is not conditional on the publisher's continued goodwill.

---

## Repo-sourced wikis

### The repository is authoritative

t[wiki.source.repo]
A wiki can mirror a path inside a git repository — a product's `docs/` becoming
a wiki. The repository is the source of truth and the wiki is a mirror of it:
the wiki's history *is* the repository's history, and no accepted change exists
in one without existing in the other. Adopting a repo moves and rewrites nothing
in it, and git, CI and an IDE go on seeing the tree they always saw.

---

### The mirror tracks the repository

t[wiki.source.sync]
Commits upstream become wiki content without a person re-importing anything, so
documentation shipped with a release is in the wiki when the release is. Which
commit the wiki reflects is visible on the wiki, and a fetch that fails says so
rather than serving stale content as current.

---

### Editing works, and landing goes through the owner

t[wiki.source.editable]
Pages of a repo-sourced wiki are editable from the app, and changes arrive as
Edit Requests like anywhere else — a contributor needs an account here, not one
on the forge. Landing a change in the repository is done by someone whose forge
account is linked: they forward an accepted request as a commit or a pull
request from that account, so the repository's own history and review remain
truthful about who pushed. A change the repository refuses is reported as
refused, and the wiki does not show it as landed.

---

### Where it came from changes nothing else

t[wiki.source.same-surface]
A repo-sourced wiki is a wiki in every other respect: subscribable, referenceable,
searchable, lintable, and present in the local tree. No surface outside the
landing path branches on whether a wiki has a repository behind it.

---

## Local presence

### Every wiki is a folder on disk

t[wiki.local.mount]
Every wiki an org owns, and every source it subscribes to, appears in the file
sync clients under `Task/Wikis/<id>` as plain markdown that any outside tool can
open. The point is possession: what matters to someone is on their disk, so it
is readable and writable with the network down and with this application closed.

There is one shape for all of them. A wiki the org owns and a wiki it subscribes
to sit side by side, both editable in place, and an edit made in an outside
editor is the same working change as one typed in the app — same history, same
push path (`wiki.subscribe.push`), no import step and no second mechanism for
edits that arrived through the filesystem.

---

## What this asks of the seed

Per `CLAUDE.md`, the planted world is part of the feature. Every rule above is
exercised from the seed through the integration suite, not from the UI alone.
The seeded world is:

| what | owner | kind | demonstrates |
|---|---|---|---|
| **Music Theory** | acme-audio | wiki, public | multiplicity; the target of a cross-wiki reference, including a block anchor |
| **Audio Production** | acme-audio | wiki, public | two wikis referencing each other both ways — one web, one owning wiki per page |
| **Bible Study** | alice-personal | wiki, private | a wiki annotating a Resource without writing into it; private is a refusal, not an absence |
| **Cooking** | alice-personal | wiki, unlisted | a personal wiki in a person's own org; unlisted rather than private |
| **Bible** | — | Resource | read-only spine, verse addressing, core membership, licence and per-passage availability |

`alice-personal` is a third org in the example, hosted on the same data root as
ACME. It is what "a person's own org" means (`wiki.boundary.role`) and it is
what makes two orgs on one server, and a personal wiki, testable at all.

The Bible Resource is **declared but not committed**. `OrgRoot::resources_dir`
already says corpora "never live in the git repo" — they install into
`resources/bible/<TRANSLATION>/` through `scripture::install_usfm_dir`. So the
seed carries the Resource's identity, addressing, licence and core membership,
and the text arrives by install. Everything anchored to a verse is exercised
without a verse of scripture being committed; what needs real text is marked
and skipped when no edition is installed.

On top of that world: the org vault subscribed to Music Theory and Audio
Production so `[[Ionian]]` resolves from a vault note; a private journal note in
`alice-personal`'s vault referencing a private wiki, a personal wiki and the
Resource, none of which can reference it back; one open Edit Request from a cast
member holding no Editor role; one auto-approved Editor change; a repo-sourced
wiki over a small committed repository; and VNT subscribed to ACME's Audio
Production across the two demo servers, so `wiki.subscribe.federated` is
exercised against a second server rather than asserted.

Scripture is in the seeded core set, so a freshly planted vault resolves a verse
reference with nothing subscribed by hand — which is the check that
`wiki.core.default` is real and not a setting someone has to find.
