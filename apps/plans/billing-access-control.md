# Billing access control + per-person visibility (deferred)

**Status:** parked 2026-06-02 ("don't need it yet"). Direction decided: use **real architect-auth users**, phased. This doc captures the investigation so we don't redo it.

## Goal
- See **personal** billing (my own time) vs **org** billing.
- As an **owner/admin** of an org (TomBrooksMusic, FastTrackStudio…), see *everyone's* time/invoices (Carter, Caleb), clearly distinguished from mine.
- **Non-owners** see only their own.

## The gap (why it's not trivial)
A full auth/membership/role model exists (`architect-auth`: `AuthUser`, `AuthMember{ role }`, `AuthSession`) but is **not wired to the timer/finance services**:
- `org_layer_router` (`apps/server/src/lib.rs:831-985`) mounts timer (`:858`) + invoicing (`:880`) with **no auth middleware** — anonymous. Only `AuthService` has `AuthServerMiddleware`, and that only parses the Bearer token (doesn't even validate/resolve a user). Token is only really validated in `server_mgmt.rs` for org *creation* (no role check).
- `TimerService` takes `user_id` as a **plain request param** (caller passes any id). `Invoicing` has **no user concept** (invoices key on `book_id`/`party_id`; `list_invoices()` returns all, unfiltered).
- CLI/UI resolve identity to one synthetic **local owner** `Uuid::new_v5(org_id, "task-local-owner")` (`apps/cli/src/main.rs:5623`, `crates/ui/src/chrome.rs:424`). The real login flow (`task auth login` → `session.json`) exists but timer/finance **ignore it** — the CLI hits SQLite directly, no vox, no token.
- **Cody/Carter/Caleb are nameless uuids** — names live only in `AuthUser.name` in `auth.sqlite`, never queried by timer/finance. "Caleb"/"Carter" appear nowhere in the code; their ids (`6009c630…`, `dbc60551…`) and the owner v5-id have no `AuthUser` rows.

## Plan
### Phase 1 — People model + owner's per-person view (display only)
1. **Seed real identities:** create `AuthUser` + `AuthMember{role}` rows in each org's `auth.sqlite` for the existing user-ids (Cody = owner, Carter/Caleb = member). Reuses architect-auth (`AuthUserEntity` — already a CLI dep).
2. **user_id → (name, role):** a read path joining session `user_id` → `auth.sqlite`. The local-owner v5-id has no AuthUser row → either create one for the real owner or label "Owner".
3. **Per-person view:** group timer/finance by `WorkSession.user_id`; reports already accept a per-user filter (`finance::reports::hours_by_project(conn, Some(user_id), range)`, `weekly_summary` — `features/finance/finance/src/reports.rs:89-205`) — just stop passing `None` and add grouping. UI: Mine / Everyone toggle on `/finances` + `/invoices` + `/timer`; label each row by person. **No enforcement.**

### Phase 2 — Real auth-enforced access control
1. Make timer/finance clients talk over **vox with the Bearer token** (today the CLI bypasses vox → no transport for auth; this is the big change).
2. Add **server middleware on the timer/finance dispatchers** that validates the token, resolves `AuthSession → user_id`, looks up `AuthMember.role` for `active_organization_id`, injects an authed identity into request extensions.
3. Services read caller identity from **context** (not a client-supplied `user_id`), and filter `list_invoices` / `list_sessions` / reports by it for non-owners; owners/admins see all.

## Key refs
`apps/server/src/lib.rs:831-985`, `apps/server/src/server_mgmt.rs:53-139`, `apps/cli/src/main.rs:5322,5619-5662,1990-2030`, `apps/cli/src/org_ctx.rs:52`, `features/timer/timer-proto/src/service.rs`, `features/finance/finance/src/reports.rs:89-205`, `~/Development/architect-auth/features/auth/auth-proto/src/{member,user,session}.rs`. Related: [[project_timer_billing]].
