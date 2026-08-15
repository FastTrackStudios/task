# architect::permissions — a framework-level authorization system

Status: IMPLEMENTED through P4 (2026-07-22) — see "Implementation notes"
at the end for where the build deviated from this design. Companion to
`collaboration-sharing.md` (the share lane is the first consumer) and
`billing-access-control.md` (whose "identity middleware" became
`SessionIdentityResolver`). Framework home: `libs/architect/permissions`
(+ `permissions/proto`), gate in `architect::permissions_gate`.

## Why in architect

Every product on the stack has the same problem shape: vox services mounted
on a `LayerRouter`, a bearer token in per-request metadata
(`AuthClientMiddleware::bearer` → `"authorization"` key), and **zero
enforcement** past "which URL did you connect to". Task needs org roles +
share scopes; signal's detachable GUIs will need "who may touch the rig";
the docs-sync CRDT layer needs per-doc write gates. One framework primitive,
three products.

## Core model

Four nouns, all Facet wire types in `architect-permissions-proto`:

```rust
/// WHO is asking. Produced by identity middleware, never by the caller.
enum Principal {
    User { user_id: String },            // validated session → AuthUser
    Guest { link_id: String,             // share-lane visitor
            display: Option<String> },
    Service { name: String },            // in-process / trusted callers
    Anonymous,
}

/// WHAT is being touched. Hierarchical, path-shaped, cheap to pattern-match.
/// Convention: `<domain>/<segment>/…` — e.g.
///   vault/Setlists/Sunday Worship.md
///   media/<content-hash>
///   doc/<doc-id>
///   threads/vault_file/<path>
///   service/finance/*
struct Resource(String);

/// The verb. Small fixed core + free-form domain extensions.
struct Action(String);        // "read" | "write" | "comment" | "admin"
                              //  | domain-specific: "download", "invite", …

/// The answer, with the WHY (surfaces in errors + audit).
enum Decision { Allow, Deny { reason: String } }
```

And one trait:

```rust
pub trait PermissionEngine: MaybeSendSync {
    fn check(&self, who: &Principal, what: &Resource, action: &Action) -> Decision;
    /// Bulk affordance query — powers UI capability manifests (see below).
    fn survey(&self, who: &Principal, prefix: &Resource) -> Vec<(Resource, Vec<Action>)> { … }
}
```

### Engines (composable, product-supplied)

- **`RoleEngine`** — org members: resolves `AuthMember.role` →
  `AuthOrganizationRole.permissions_json` (both already exist in
  `architect-auth`); the permissions blob is a list of
  `(resource glob, actions)` rules. Default roles ship as constants
  (`owner`, `member`, `guest`).
- **`ScopeEngine`** — a materialized allowlist: `Vec<(resource-prefix,
  actions)>`, built per share-link from the expanded `ShareScope`
  (collaboration-sharing.md §1). Constructed per lane, cheap, no DB reads
  on the hot path; rebuilt when the scope or link settings change.
- **`Composite`** — first-match-allow over `[deny-list, RoleEngine,
  ScopeEngine]`; also where org-level kill-switches (billing suspensions)
  slot in.

## Wiring — three enforcement points

### 1. Method annotations on `#[architect::rpc]` traits

The macro grows a per-method attribute declaring the action and how to
derive the resource from the request:

```rust
#[architect::rpc]
pub trait VaultSync {
    #[permit(action = "read", resource = "vault/{path}")]
    async fn get_file(&self, path: String) -> Result<FileBody, VaultError>;

    #[permit(action = "write", resource = "vault/{path}")]
    async fn put_file(&self, path: String, body: FileBody, if_match: IfMatch)
        -> Result<(), VaultError>;
}
```

`{path}` interpolates from the (Facet-reflected) request fields. The
generated dispatcher, when the router has an engine installed, runs
`engine.check(principal, resource, action)` BEFORE the handler and returns a
typed `PermissionDenied` error on Deny. Methods without `#[permit]` on a
permissioned router default to **deny** (fail-closed) — a lint flags them.

### 2. Router-level installation

```rust
LayerRouter::new()
    .with(vault_descriptor(), vault_dispatcher)
    …
    .with_identity(AuthIdentityMiddleware::new(auth))   // token → Principal
    .with_permissions(engine)                            // Principal × permit → gate
```

- `AuthIdentityMiddleware` is the upgraded `AuthServerMiddleware`: it
  VALIDATES the bearer token (`current_session`), resolves the `Principal`,
  and injects it into request extensions (today it only stuffs the raw
  token). This is exactly billing-access-control.md's S1, landing in
  architect instead of Task.
- `.with_permissions(None)` (the default) keeps today's behavior —
  migration is opt-in per router, product by product.
- The **share lane** is now trivial: same service impls, a router built
  with `Principal::Guest` pre-bound and a `ScopeEngine` — no per-service
  wrapping code at all (supersedes the hand-rolled "scoped VaultSync"
  wrappers sketched in collaboration-sharing.md §3b).

### 3. In-handler fine-grained checks

For decisions the method signature can't express (e.g. MediaService `read`
where the hash→scope mapping needs the share's hash set):

```rust
async fn read(&self, ctx: Ctx, hash: String, tx: Tx<MediaChunk>) -> … {
    ctx.permissions().require(res!("media/{hash}"), Action::READ)?;
    …
}
```

`ctx.permissions()` is the same engine + principal from extensions, so
handler checks and dispatcher checks can never disagree.

## The capability manifest (UI affordances)

Clients shouldn't discover permissions by getting errors. On lane
establish (and on any permission change), the server pushes a
**capability manifest** — `engine.survey(principal, "")` distilled to
`(resource-prefix, actions)` pairs — over a `#[subscribe]` stream on a tiny
`PermissionsService`:

```rust
#[architect::rpc]
pub trait Permissions {
    /// One-shot check (rarely needed client-side).
    async fn can(&self, resource: String, action: String) -> bool;
    /// The live affordance set for THIS principal on THIS lane.
    #[subscribe]
    fn capabilities(&self) -> CapabilityManifest;
}
```

The UI greys/hides affordances from the manifest (no Edit button on a View
share; no download icon when `allow_download` is off), and a permission
flip mid-session re-renders live — the retroactivity story from
collaboration-sharing.md falls out of this stream.

## Auditing

`with_permissions` takes an optional `AuditSink`: every Deny (always) and
selected Allows (`audit = true` on `#[permit]`, e.g. downloads) append
`(ts, principal, resource, action, decision)` — the share panel's activity
feed and the org audit log read this.

## What lives where

```
libs/architect/permissions/            engine trait, Principal/Resource/Action,
                                       RoleEngine/ScopeEngine/Composite, AuditSink
libs/architect/permissions/proto       wire types + PermissionsService trait
libs/architect/macros                  #[permit] parsing + dispatcher codegen
libs/architect/auth                    AuthIdentityMiddleware (validating upgrade
                                       of AuthServerMiddleware)
apps/task/server                       engines constructed per org router +
                                       per share lane; manifest push wiring
```

## Staging

1. **P1 — crate + trait + engines**, no codegen: `PermissionEngine`,
   `RoleEngine` over `permissions_json`, `ScopeEngine`, `Composite`, unit
   tests (glob matching, precedence, fail-closed).
2. **P2 — identity upgrade**: `AuthIdentityMiddleware` validates tokens →
   `Principal` in extensions (replaces the token-stuffing
   `AuthServerMiddleware`); Task org routers install it. Ships alone =
   billing-access-control S1.
3. **P3 — `#[permit]` codegen + `with_permissions`**: macro + dispatcher
   gate + fail-closed lint; annotate VaultSync, MediaService, DocSync,
   ThreadsService, ShareService first (the share-lane surface); org lane
   runs `RoleEngine` with permissive defaults (owner/member = allow-all)
   so nothing user-visible changes.
4. **P4 — PermissionsService + capability manifest** stream; UI consumes it
   for affordance rendering.
5. **P5 — share lane on ScopeEngine** (replaces collaboration-sharing S2's
   hand-scoped services); audit sink + activity feed.

## Non-goals (for now)

- Row-level DB policy (SQLite RLS-alikes) — resource paths + engines cover
  the products' needs.
- Cross-org / federated policy exchange — federation identity mapping
  (`federated-task-platform.md` Phase 3) plugs in as a Principal resolver
  later, not a new engine.
- A policy DSL — `permissions_json` stays plain `(glob, actions)` rules
  until real demand says otherwise.

## Implementation notes (what actually landed, 2026-07-22)

- `libs/architect/permissions` — Principal/Resource/Action/Decision,
  `PermissionEngine` (+ `survey`), glob matching, `RoleEngine` (JSON role
  blobs, `with_default_user_role`), `ScopeEngine`, `CompositeEngine`,
  `MethodPermit`/`ServicePermits`, `AuditSink`. Unit-tested.
- `libs/architect/permissions/proto` — `CapabilityManifest` +
  `PermissionsService` (`can`, `capabilities`) with the `Permissions<E, I>`
  impl. The `#[subscribe]` manifest stream is still future work (needs
  share mutations to publish from).
- **Gate = a wrapping `Handler`, not `#[permit]` macro codegen** (P3
  deviation): vox `ServerMiddleware` can observe but not refuse, so
  enforcement lives in `architect::permissions_gate::PermissionedRouter<H>`
  — method-granular via `ServicePermits` tables resolved against service
  descriptors, fail-closed for unlisted methods of tabled services,
  `UnlistedPolicy` for untabled services, observe-only rollout mode.
  Argument-level `{path}` checks remain the service impl's job (same
  engine, finer resource) — the `#[permit]` macro sugar can still come
  later and compile down to exactly this.
- **Denies are encoded in the method's real response wire shape**
  (`response_wire_shape` + facet-reflect `Partial` dynamic construction of
  `Err(VoxError::InvalidPayload("permission denied: …"))`), so clients
  decode the reason verbatim; falls back to type-erased `send_error`.
- `auth::identity::SessionIdentityResolver` — VALIDATING token → Principal
  (real `current_session`, 30 s TTL cache). The upgrade of the
  token-stuffing `AuthServerMiddleware`; billing-access-control S1 is done
  in substance.
- Task org lane wired (`apps/task/server`): per-org
  `PermissionsGate` (role engine with default-role `member` for any
  validated user — per-row membership sync arrives with shares), permit
  tables for VaultSync + MediaService, `PermissionsService` mounted from
  the gate's own engine/resolver, gate wrapped OUTSIDE the snapshot gate
  per connection. **Observe-only by default**; `TASK_ENFORCE_PERMISSIONS=1`
  enforces. Schema stamp added.
- E2E tests (`auth-native-tests::permissions_gate_tests`): real sign-up →
  bearer → allow/deny/fail-closed/forged-token over a vox memory link;
  share-lane ScopeEngine + StaticPrincipal guest; observe-only pass-through.
