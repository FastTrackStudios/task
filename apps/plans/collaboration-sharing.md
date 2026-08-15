# Collaboration panel — sharing note groups with people and links

Status: DESIGN (2026-07-22). Substrate audit done; nothing implemented.
Companion plans: `architect-permissions.md` (the framework
PermissionEngine / `#[permit]` system that supersedes this doc's hand-scoped
share-lane services — the share lane becomes ScopeEngine + Guest principal),
`billing-access-control.md` (the enforcement middleware this depends on),
`multi-server-auth.md` (client tokens — done, server enforcement open),
`federated-task-platform.md` (external collaborators as federated members).

## Product goal

Share a **group of notes** — not the whole org — with specific people (email /
contact) or via a **public link**, at a chosen capability (**view / comment /
edit**), rendered through a **purpose-tuned landing experience**:

- **Band**: share `Setlists/Sunday Worship.md` → recipients open the browser
  setlist player and rehearse — stems stream, charts render, transport works.
- **Orchestra**: share an Alan Parsons show setlist → each musician picks (or
  is pre-assigned) their instrument and downloads their part PDFs for every
  song in the set, one file at a time or as one zip.

The same Share powers both; the landing page adapts to what the scoped
content contains (stems → practice player; orchestral-parts → parts desk).

## 1. Data model — the `Share`

New per-org table (home: `auth.sqlite`, beside the auth entities it joins):

```
Share {                         // ONE scope, many audiences/links
  id:            ShareId (uuid)
  title:         String        // "Sunday Worship — band rehearsal"
  scope:         ShareScope    // see below
  created_by:    user_id
}
ShareLink {                     // Samply-style: a share can mint MANY links,
                                // each with its OWN settings, individually
                                // editable + revocable after creation
  id, share_id
  token:         String        // unguessable URL secret
  label:         String        // "band link", "orchestra front desk", …
  capability:    View | Comment          // link-based Edit is not offered;
                                          // Edit is always a named grant
  experience:    Auto | Practice | PartsDesk | Notes
  allow_download: bool         // playback/preview WITHOUT download (streams
                               //  stems / previews PDFs, no file export)
  password_hash: Option<String> // optional extra gate on the link
  expires_at:    Option<ts>
  disabled_at:   Option<ts>    // deactivate (reversible) — delete is separate
  created_by, created_at
}
ShareGrant {                    // named audience members (email / contact)
  share_id, email, invitation_id: Option<AuthInvitationId>,
  user_id: Option<user_id>,     // filled once accepted / signed in
  capability: View | Comment | Edit,
  instrument: Option<String>,   // orchestra flow: pre-assign "Violin 1", …
}
```

**Every setting is retroactive by construction**: the share lane resolves
`token → ShareLink → Share` on every connect/request, so flipping a link
from Comment to View, toggling `allow_download`, adding a password, or
deactivating it applies to the next request — including live-connected
guests (the lane re-checks on reconnect; active WebSocket lanes are dropped
on disable/permission change).

### Scope: a note group, expanded

```
ShareScope {
  roots:   Vec<VaultPath>,      // explicitly selected notes/folders
  expand:  bool,                // follow structural references (default true)
}
```

Expansion is **server-side and deterministic**, recomputed on access (so a
setlist edit updates what recipients see):

- `type: setlist` note → its `songs:` slugs → `Songs/<Title>.md` notes →
  `resources/songs/<slug>/**` (manifest, chart, stems) and
  `resources/orchestral-parts/<slug>/**`, `resources/scores/<slug>/**`.
- A folder root → its subtree.
- Plain note → itself + its attachments.

The expanded set materializes as a **path-prefix allowlist** — the unit every
enforcement point checks. (This keeps ACLs out of the per-note metadata; a
share OWNS its scope, notes know nothing about being shared.)

## 2. Audiences and identity

Three kinds of recipient, in increasing weight:

1. **Public-link visitor** — no account. The share `token` IS the credential.
   First visit prompts a display name (stored client-side + echoed into
   presence/comments as "Guest · Sarah"). Capability capped at Comment;
   Edit always requires a named grant.
2. **Email invitee** — reuses `AuthInvitation` (already in `architect-auth`
   with `create_invitation` / `accept_invitation` flows — currently just not
   exposed on the vox `AuthService`; adding the trait methods is documented
   as purely additive). The invite email carries the share URL + invitation
   token; accepting creates/links an `AuthUser` and fills
   `ShareGrant.user_id`. These are **share-scoped members**: an
   `AuthMember` row with role `"guest"` so they never gain org-wide access.
3. **Federated collaborator** (later) — a band member with their own home
   org accepts via the identity-link flow in `federated-task-platform.md`;
   `ShareGrant.user_id` points at the federated member row.

## 3. Enforcement — the honest prerequisite

Today **nothing but `AuthService` even parses the bearer token**; every other
service on the org router is anonymous, and org scoping is only "which URL
you connected to". Sharing CANNOT ship before this is closed. Two layers:

### 3a. Identity middleware (from `billing-access-control.md`, unchanged)

Vox server middleware on every dispatcher: validate `Bearer` token →
`AuthSession` → `user_id` → `AuthMember.role`; inject `AuthedIdentity` into
request extensions. Members of the org pass everything (status quo UX);
unknown/absent token → the request is only satisfiable via a share lane.

### 3b. The share lane — a scoped router, not per-method ACLs

Retrofitting allowlist checks into ~50 service impls is a losing game.
Instead, guests connect to a **dedicated endpoint**:

```
/org/{slug}/share/{token}/vox        (WebSocket, same wire protocol)
/org/{slug}/share/{token}/…          (HTTP: landing page, zip download)
```

The handler resolves the token → live `Share` (not revoked/expired) → builds
a **restricted `LayerRouter`** that mounts ONLY share-safe services, each
wrapped with the share's path allowlist + capability:

- `VaultSync` (scoped): `manifest`/`get_file`/`subscribe` filtered to the
  allowlist; `put_file` only when capability = Edit and path ∈ scope.
  (This is the path-prefix scoping the vault layer already anticipates —
  `default_client_vault_root` "thin client mounts a slice", `mount`'s
  `Backend::list(prefix)`.)
- `MediaService` (scoped): `info`/`read` only for content hashes referenced
  by manifests/notes inside the scope. Resolution: the share lane keeps the
  expanded scope's hash set (recomputed with the scope), so stems and part
  PDFs stream over the SAME per-org vox lane pattern the app already uses.
- `DocSync`/`DocPresence` (scoped, Edit shares only): doc ids restricted to
  `collab_doc_id(vault_id, path ∈ scope)` — live co-editing on exactly the
  shared notes.
- `ThreadsService` (scoped, Comment+): anchors restricted to
  `(entity_type = "vault_file", entity_id ∈ scope)`. The threads feature's
  polymorphic anchor already supports this with no schema change.
- Session/setlist engine: NOT mounted — the practice player runs its engine
  in-browser (in-process wasm engine), needing only vault + media. Nothing
  server-side to scope.

Everything else (agents, finance, timers, forge, …) simply isn't mounted on
the share lane. The blast radius of a leaked link is the scope, period.

Share tokens: random 128-bit ids stored server-side (revocable rows), NOT
self-authenticating signed tokens — revocation and per-share state matter
more than statelessness here. (The Ed25519 `ServerKeypair` stays for signed
blob URLs; the ripped capability/share-link service is effectively being
rebuilt as this share lane.)

## 4. Landing experiences

`https://<server>/org/{slug}/share/{token}` (or `task.…/s/{token}` alias)
serves the web app shell in **share mode**: the wasm app detects the share
context from the URL, connects its vox client to the share lane, and renders
by `experience`:

- **Auto**: scope contains `resources/songs/**` manifests → Practice; only
  `orchestral-parts/**` → PartsDesk; else Notes.
- **Practice** (band): the existing fullscreen setlist experience
  (`setlist_session.rs`) — navigator, charts, mixer, transport — fed by the
  scoped VaultSync + MediaService. View capability = play + mix locally;
  Comment adds the section-anchored comment rail; Edit unlocks the chart
  editor (DocSync).
- **PartsDesk** (orchestra): instrument picker (pre-selected from
  `ShareGrant.instrument` when invited by email) → table of songs in setlist
  order × that instrument's PDFs (from `orchestral-parts/<slug>/`), inline
  PDF preview, per-file download, and **"Download my book (zip)"**:
  `GET /org/{slug}/share/{token}/zip?instrument=Violin%201` — a new HTTP
  route that streams a zip of the matching PDFs in setlist order with
  numbered filenames (`03 TIME - Violin 1.pdf`). (No zip machinery exists
  anywhere yet; this is its first home. `zip` crate is already in-tree via
  keyflow-musx.)
- **Notes**: plain read/comment/edit view of the shared notes.

## 5. Comments

`threads` anchors `(entity_type: "vault_file", entity_id: <path>)`, plus a
finer anchor for the player: `(entity_type: "setlist_position",
entity_id: "<path>#<song-slug>@<section>")` so a band member can write
"push the tempo here" ON a section. Guest authorship: `ShareGrant.user_id`
when named, else `share:{share_id}:{display_name}` — rendered with a guest
badge. Threads-ui already exists for rendering.

## 6. The panel (UI)

New `RightTab::Share` beside Properties/Links in `vault.rs` (the note's
right-hand panel), following the atom-store pattern:

```
┌ Share ──────────────────────────────┐
│ Scope: this note + 12 linked        │
│   [Sunday Worship] → 6 songs,       │
│   stems, charts        [customize]  │
│                                     │
│ People                              │
│  sarah@band.com      Edit    ▾  ✕   │
│  strings@symphony.org View · Vln 1  │
│  [+ invite by email]                │
│                                     │
│ Links                    [+ new]    │
│  band link      Comment · no dl     │
│   https://task…/s/8Xk2…  [copy] ⋯   │
│  orchestra desk View · dl · 🔒      │
│   https://task…/s/P9q4…  [copy] ⋯   │
│   (⋯ = edit · deactivate · delete)  │
│                                     │
│ Landing: [Auto ▾]  (Practice)       │
│ Activity: 3 visitors this week      │
└─────────────────────────────────────┘
```

- Capability per grant (dropdown), instrument field appears when the scope
  contains orchestral parts.
- "+ new link" mints another `ShareLink` with its own label/settings —
  a share can carry several at once (band link with downloads off,
  orchestra link with downloads on, a view-only promoter link…).
- Per-link controls (Samply-style): capability, downloads on/off, password,
  expiry, **deactivate** (reversible) vs delete, copy URL.
- Invite-by-email goes through the newly-exposed vox
  `create_invitation` + `ShareGrant`; Members page (`pages/members.rs`)
  grows the same invite affordance for org-level membership.
- Share management RPC: new `ShareService` (create/update/revoke/list,
  links CRUD, list_grants, activity) mounted on the ORG lane (members only).

### The Links registry (org-level)

A dedicated **Links** page (sidebar, beside Members): every link ever
created across the org, in one table —

```
Label            Scope              Cap      Dl  Status    Last visit  Visits
band link        Sunday Worship     Comment  ✕   active    2h ago      14
orchestra desk   Columbus Symphony  View     ✓   active    yesterday   31
promoter copy    Columbus Symphony  View     ✕   disabled  Jun 30      2
```

Row actions: edit settings inline, copy, deactivate/re-enable, delete,
jump to the owning note's Share panel. This is the "keep track of every
link that's been created and change permissions after the fact" surface —
nothing is fire-and-forget.

## 7. Staged plan

1. **S1 — enforcement middleware** (`billing-access-control.md` S1): token →
   identity on every org-lane dispatcher; org members pass. Ships alone.
2. **S2 — Share model + share lane (View)**: `Share`/`ShareLink`/
   `ShareGrant` tables, `ShareService` (links CRUD — settings mutable and
   retroactive from day one), `/org/{slug}/share/{token}/vox` with scoped
   VaultSync + MediaService, scope expansion for setlist notes, the Links
   registry page. Landing = Practice/Notes (view). `allow_download` off ⇒
   stems stream / PDFs preview but no file export; the zip route and raw
   downloads check it. *Band-rehearsal link works end-to-end here.*
3. **S3 — PartsDesk + zip route**: instrument picker, per-song part listing,
   zip streaming endpoint. *Orchestra flow done.*
4. **S4 — Comment**: scoped ThreadsService on the share lane, guest
   authorship, comment rail in the player + notes view.
5. **S5 — email invites**: expose `create_invitation`/`accept_invitation`
   over vox, invite emails (needs outbound mail — first in the product;
   SMTP config on the server), grant acceptance → `user_id` fill,
   per-grant instrument.
6. **S6 — Edit**: scoped DocSync/DocPresence on the share lane, guest
   presence badges, Edit capability in the panel (named grants only).

## 8. Security notes

- Public link = bearer secret in a URL: cap at Comment, default View;
  always revocable (server-side row), optional expiry; rate-limit the
  share endpoints; don't index (`X-Robots-Tag: noindex`).
- Scope recomputation must be the ONLY authority — never cache an expanded
  allowlist beyond a request without invalidation on vault change.
- `DEFAULT_AUTH_SECRET` / `DEV_ACCOUNTS` (dev-only) must be dead in prod
  before any of this ships (flagged in `multi-server-auth.md`).
- Zip route must enforce the same allowlist as the lane (same resolver).
- Audit trail: append share-lane connects + downloads to a per-share
  activity log (the panel's "3 visitors this week").
