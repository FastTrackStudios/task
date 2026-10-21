# Sharing Spec

Cross-org project sharing: an org that owns a project lets ONE person
from another org see that project's subtree, read + comment, without
making them a member. The first case is Crescendum — owned by
FastTrackAudio, worked on with people from Rockstars of Tomorrow — and
the rules below are written for it but name nothing specific to it.

The owner keeps the only copy. The guest reaches it online, through a
lane of its own that admits them by their grant on every call, and the
guest's own org shows a reference to it rather than a copy.

The pieces:

| piece | where |
|---|---|
| the wire contract — `SharingGrantInfo`, `NewSharingGrant`, `GrantPreview`, `GrantCapabilities`, `#[architect::rpc] trait SharingService { preview_grant, create_grant, revoke_grant, list_grants }` | `features/share/share-proto/src/grant.rs` |
| the grant rows and their audit lines — `<org>/sharing.sqlite`, `GrantStore`, `is_live` | `apps/server/src/sharing/store.rs` |
| what a grant exposes, and the preview digest — `Subtree`, `digest` | `apps/server/src/sharing/subtree.rs` |
| the owner's service — owner/admin check, refusals, audit | `apps/server/src/sharing/mod.rs` |
| the gate entry — `sharing/**` | `apps/server/src/permits.rs` |
| the guest's lane — `POST /org/{owner}/grant/{grant_id}/vox`, its allowlist and mounts | `apps/server/src/sharing/lane.rs` (to come) |

## The grant

A **Sharing Grant** is one row in the owning org's store:

| field | meaning |
|---|---|
| `id` | the grant's uuid; the guest lane is addressed by it |
| `owner_org` | the owning org's slug — the org whose store holds the row |
| `root_id` | the project whose subtree is shared |
| `subject` | the person: an fts-auth user uuid. Holds no membership row in the owner org |
| `guest_org` | the subject's own org (slug) — metadata: where the shared-in reference shows. Grants nothing there |
| `capabilities` | `ReadComment` in v1 |
| `expires_at` | unix seconds, optional; at or after it the grant is not live |
| `granted_by`, `granted_at` | the owner or admin who made it, and when |
| `revoked_at`, `revoked_by` | set together, once, by the revoke |
| `preview_digest` | the digest of the visible set the owner confirmed |

A row is written once and changes once at most: the revoke. The store
enforces this itself — an update that touches any other column, or a
row already revoked, aborts, and so does every delete. A change of
scope is a revoke and a new grant. A grant is **live** while it is
unrevoked and unexpired (`sharing::is_live(grant, now)`), and that is
the only definition of live the lanes use.

---

## Who grants

### Owner only

t[sharing.grant.owner-only]
Only an owner or admin of the owning org — as that org's membership row
says — creates, revokes, lists or previews a grant. A plain member, a
signed-in person with no membership row there, a grantee, a bot and an
anonymous caller are all refused, and the refusal reads as absent
(`t[sharing.grant.absent]`). A grantee never grants, invites, or shares
a link onward.

---

### Preview, then grant

t[sharing.grant.preview]
Before a grant exists the owner is shown the exact set it would make
visible, with the classes it never carries listed beside it ("not
shared: pricing, invoices, timer, contacts"), and the set's digest. The
grant is written only with the digest the owner confirmed; if the set
has changed since the preview, creating it is refused with the current
digest, and the owner previews again. The grant stores the digest it
was made with.

---

## What a grant reaches

### The subtree

t[sharing.grant.subtree]
A grant on a project reaches the project, its sub-projects at any depth,
its parts, the tasks whose project is in the subtree, the File Roots
adopted from its directories, and the vault notes under its directory.
The subtree is evaluated on every call, so what is added to it later is
shared with no new grant and what leaves it is unshared at once.

---

### No membership

t[sharing.grant.membership-free]
The subject holds no membership row and no role in the owning org, so
on every membership-gated route their baseline is empty. A grant is
reached only through the grant lane, never through the org lane, the
org's MCP route or the account lane.

---

### Default deny

t[sharing.grant.default-deny]
The grant lane mounts an allowlist of services. A service not listed is
absent on that lane — including every service added to the server after
the list was written.

---

### Absent, never forbidden

t[sharing.grant.absent]
Anything outside a grant reads to the guest exactly as a thing that does
not exist: the same answer, the same shape, never "forbidden". A caller
cannot learn from a refusal that there was something to refuse.

---

### Never in scope

t[sharing.grant.exclusions]
Money — the timer, finance, a project's `client_id`,
`default_rate_cents` and budget — other projects, contacts, and the
org-level feeds are never in a grant's scope, whatever the subtree
holds. A link from an in-scope object to one of them resolves to
nothing.

---

## What a guest does

### Comments, attributed

t[sharing.grant.attribution]
Every write a guest makes — in v1, a comment — carries
`created_by = <the guest>` and `via_grant = <the grant id>`. Both are set
by the server from the caller the grant lane admitted, and never taken
from the call's arguments.

---

### Read and comment only

t[sharing.grant.comment]
The only write a v1 grant allows is a comment on an object in scope.
Every other write is absent on the grant lane.

---

### Revoke is one action

t[sharing.grant.revoke]
Revoking a grant is one action by the owner. It takes effect on the
guest's next request and on every stream the guest has open, which the
server closes.

---

### Online only

t[sharing.grant.online-only]
No guest device holds a copy of shared content in v1: no replica, no
files-sync lane, no offline store is mounted for a guest.

---

### Shared in

t[sharing.grant.shared-in]
The guest org's project list shows, to the grant's subject, a reference
to the shared project — `{owner_org, root_id, grant_id}`, derived from
live grants at read time. No project field is copied into the guest
org, and the reference is gone at the first list after the grant is
revoked or expires.

---

## Audit

t[sharing.grant.audit]
Creating, revoking and the expiry of a grant are each recorded once,
with the subject, the root, the capabilities, the preview digest and
the person who acted (none for an expiry). Every guest call and every
guest refusal on the grant lane is recorded with the principal, the
grant and the service.

---

## In the seed

The planted world carries one of everything above, so the lanes can be
reached from it (`docs/demo.md`, "Crescendum — shared from
FastTrackAudio"). FastTrackAudio owns **Crescendum**
(`9880bdc8-307f-4ed6-b6cb-6e05ca733571`): two songs, one with a
sub-project nested a level deeper, parts, tasks, notes and a File Root,
and a client and a rate on its page. One live `ReadComment` grant on it
names Jordan, a person of Rockstars of Tomorrow with no account and no
membership row in FastTrackAudio; there is no guest bot. Beside it,
**Northbeam Jingles** holds what no grant reaches: its own tasks,
notes and File Root, a client contact, a rate, billed time and an
invoice. `example_org::SEED_GRANTS`, `DECLARED_CLIENTS` and
`SEED_BILLING` declare them; `example_org::declared_tests` and
`demo_plant.rs` hold them true.

---

## Not in v1

Named here so nobody builds toward them by accident:

- **A guest-bot MCP lane.** Guests are people signed in through
  fts-auth. Bots acting for other people are out of scope; the owner's
  own bots reach shared projects through their ordinary membership. No
  MCP route admits a grant: `/org/{slug}/mcp` and `/mcp` stay
  membership-gated.
- **Guest-node replicas.** No guest device syncs or stores a copy
  (`t[sharing.grant.online-only]`).
- **`Subject::Remote`.** A grant's subject is an fts-auth person the
  owning server resolves through central identity, not a remote
  principal minted by another server.
- **`read_write`.** `GrantCapabilities` has one variant, `ReadComment`.
  A wider grant would be a new variant and a new rule, never a widening
  of this one.
