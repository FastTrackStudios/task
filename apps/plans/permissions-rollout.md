# Turning the permissions gate on

**Status:** in progress — coverage landed 2026-07-27 (70/70 services, 388/388
methods tabled); enforcement is still OFF everywhere and stays off until an
operator walks the steps below.

Companion to [`architect-permissions.md`](architect-permissions.md) (the
design) and [`collaboration-sharing.md`](collaboration-sharing.md) (the share
lane). This doc is the *operational* half: what to set, in what order, how to
know it is safe, and how to get back.

## What exists now

`apps/task/server/src/permits.rs` carries a `ServicePermits` table for
**every** service `org_layer_router` mounts. Before this, two services
(`vault-sync`, `media`) had tables and ~68 passed the gate unchecked — and
nothing said so at boot, which is why it stayed that way.

Three things make the state visible:

1. **Boot log, once per org.**

   ```
   INFO task_server::permits: permissions gate: 70/70 services have permit
        tables (388/388 methods) org=<slug> mode="observe-only" services=70
        tabled=70 methods=388 permitted=388 member_allowed=388
        anonymous_allowed=9
   INFO task_server::permits: permissions gate dry-run: reachable without a
        session (public surface) methods=AuthService/…, PermissionsService/…
   ```

   Anything untabled is a `WARN` naming the services. Methods missing from
   their service's table are a `WARN` (they are **fail-closed** once
   enforcing). A permit naming a method that does not exist is an `ERROR`
   (dead rule + a silently fail-closed real method).

2. **`task doctor`** prints the same coverage summary offline — the CLI links
   `task_server`, so it answers for the binary in front of you.

3. **`GET /server/permissions`** on a running server (bearer:
   `TASK_BACKUP_GIT_TOKEN`, 503 if that is unset) returns coverage, the static
   dry-run, and the tally of denials the process has actually observed.

## The two things that would break

Read these before touching the env var.

**a) Clients that never attach a bearer token.** The gate resolves identity
from the `authorization` request metadata. `RoleEngine` denies
`Principal::Anonymous` everything, so any client whose vox lane does not push
the token gets refused the moment enforcement is on — including a client that
authenticates perfectly well by passing a token as a method *argument*. There
are ~117 `establish_for` sites across the tree; this doc does not assume they
are all wired. **The observe-only log is how you find the ones that are not.**

**b) A method missing from its table.** Registering a table makes that
service's unlisted methods fail-closed. Coverage is complete today and
`tests/permits_cover_router.rs` fails the build if a new mount arrives without
a table — but a stale *deployed* binary can still differ from your checkout,
so read the running server's report, not your local one.

Two things that will NOT break: `admin`-tier permits (there are none — see
below) and sign-in (`AuthService` + `PermissionsService` are tabled against
`public/**`, which every principal holds).

### Why no `admin` permits

The org lane builds `RoleEngine::with_default_user_role("member")` and nothing
calls `set_member`. So **every validated user is a `member` and nobody is an
`owner`.** A permit requiring `admin` would deny every human on the server.
Until per-row membership sync lands (owners actually assigned), the tables
stay inside `read`/`write`/`comment`/`download`. `permits::tests::no_admin_permits`
enforces that.

The corollary: enforcement today is a check on *authentication*
(is there a valid session?), not yet on *authorization tiers*. That is still
the valuable half — it closes ~68 services that anyone who can reach the
socket may currently call.

## Rollout

### 0. Before anything — confirm the defaults

A server with none of these vars set behaves exactly as it did before permit
tables existed:

| Var | Unset ⇒ | Code |
|---|---|---|
| `TASK_ENFORCE_PERMISSIONS` | observe-only; the gate refuses nothing | `enforce_permissions()` → `observe_only(!enforce)` |
| `TASK_CORS_ALLOWED_ORIGINS` | `CorsLayer::permissive()` + a startup WARN | `cors_layer()` |
| `TASK_BACKUP_GIT_TOKEN` | `/server/permissions` is 503 | `snapshot::check_backup_auth` |

`tests/permissions_observe_only.rs` asserts the first row end-to-end: an
unauthenticated client still gets a real answer out of a tabled service.

### 1. Restrict CORS (independent, do it first)

```
TASK_CORS_ALLOWED_ORIGINS=https://task.starcommand.live
```

Cheap, reversible, and unrelated to the gate. Verify: the startup WARN about
permissive CORS disappears and an `INFO ... CORS restricted` line replaces it;
the web app still loads and signs in.

Roll back by unsetting the var.

### 2. Watch observe-only for a full usage cycle

Leave enforcement off. Collect, over a period that exercises every client you
care about (web, desktop, iOS, watch, CLI, MCP, agents, the forge poller —
and, for Task-as-a-platform, at least one Sunday):

```
journalctl -u task-server | grep 'permissions gate: WOULD DENY'
```

Each line carries the principal, the reason, and — the part the stock
`TracingAudit` throws away — the `service/method`:

```
WARN task_server::permissions: permissions gate: WOULD DENY (allowed
     through — enforcement is off) mode="observe-only" default_role="member"
     reason="permission denied: anonymous is not a member (vault-sync/manifest)"
```

Or ask the server for the aggregate instead of grepping:

```
curl -H "Authorization: Bearer $TASK_BACKUP_GIT_TOKEN" \
     https://<host>/server/permissions | jq .observed_denials
```

**The bar to clear: `observed_denials.top` contains nothing you cannot
explain.** Expected entries are unauthenticated probes and scanners. Anything
naming a service your own app uses is a client that is not sending its token —
fix the client, redeploy it, and reset the window (the tally is in-memory;
restarting the server clears it).

### 3. Sanity-check the static dry-run on the RUNNING binary

```
curl -H "Authorization: Bearer $TASK_BACKUP_GIT_TOKEN" \
     https://<host>/server/permissions | jq '.coverage, .dry_run'
```

Require: `coverage.complete == true`, `dry_run.member_denied == []`,
`dry_run.fail_closed == 0`, and `dry_run.anonymous_allowed` containing only
`AuthService/*` and `PermissionsService/*`. If `member_denied` is non-empty,
**stop** — an ordinary signed-in user would be refused those, and enforcing
would lock them out.

### 4. Flip it, on one org / one host, at a low-traffic moment

```
TASK_ENFORCE_PERMISSIONS=1
```

Restart. Expect the boot line to read `mode="ENFORCING"`. Then, immediately:

- sign in from the web app,
- open a vault note, edit it (DocSync), play a stem (media),
- run one `task` CLI command,
- check `journalctl … | grep 'permissions gate: DENY'` for anything new.

Denials now reach clients as `VoxError::InvalidPayload("permission denied: …
(service/method)")` — the reason is verbatim in the client's error, so a user
report is directly diagnosable.

### 5. Roll back

Unset `TASK_ENFORCE_PERMISSIONS` (or set it to anything other than `1`) and
restart. There is no persisted state to unwind: the gate reads the env once at
boot and permit tables are compiled-in constants. Rollback is one restart, and
it is total.

## Follow-ups (not blockers for the above)

- **Per-row membership sync** — `set_member` from `AuthMember` rows, so
  `owner` actually exists and `admin`-tier permits become writable. Until
  then the `member` role is a flat grant.
- **`UnlistedPolicy::Deny`** — with coverage at 70/70 and a test guarding it,
  the gate could fail closed on unknown services too. Deliberately NOT done in
  the same change as enforcement: one behavioural switch at a time.
- **Argument-level checks** — the tables use coarse resources (`tasks/**`);
  per-row checks (`tasks/{id}` against a real id) belong in the service impls
  calling `PermissionEngine::check` directly, as vault/media already do for
  paths.
- **Upstream `AuditEvent`** (`libs/architect/permissions`) carries no
  `service`/`method`/`role` fields, and the observe-only "would-deny" marker
  is recorded with `allowed: true` — which is why the stock `TracingAudit`
  logs it as an allow and drops the reason. `permits::GateAudit` works around
  it by keying off the marker's `resource`/`action`. The clean fix is
  structured fields on `AuditEvent`; it needs an architect change, so it was
  left alone here.
