# Multi-server connections + real accounts (+ watch config inheritance)

Goal: the app (desktop/phone/web) lets you **add a server by URL** (e.g.
`task.starcommand.live`), **log in with a real architect-auth account**, stay
logged in across launches, and connect to more than one server. The **watch
inherits the connected iPhone's active server config** instead of manual entry.

Status: planned 2026-07-18. The watch app + server HTTP bridge already ship and
are verified (commits 7aad7a80a, 7dc67d0a5); today the watch is configured
manually with a static `TASK_WATCH_TOKEN`. This plan replaces that with real
accounts + inherited config.

## Key finding — mostly wiring, not building

- **architect-auth** already exposes on the mounted per-org vox `AuthService`
  trait (`libs/architect/auth/auth-proto/src/service.rs`): `sign_up_email_password`
  (open, self-serve), `sign_in_email_password`, `current_session` (validate),
  `refresh_session`, `whoami`, `sign_out`, `list_org_members`.
- **Client session machinery** in `crates/task/ui/src/auth.rs` is real
  (`resolve_session`/`whoami`/real tokens). Only the `DEV_ACCOUNTS` password
  lookup + the one-click account-picker UI are dev-coupled.
- **`architect_auth::client::FileTokenStore`** (atomic, 0600) already does native
  token persistence — proven in the Task CLI (`apps/task/cli/src/session_store.rs`).
- **`server_registry.rs`** `ServerEntry { id, label, server_url, session_token,
  my_user_id, my_email }` has every field; it's just completely unconsumed.
- **`AuthClientMiddleware::bearer(token)`** (`auth/src/transport.rs:889`) is the
  client middleware to attach `Authorization: Bearer` to vox calls.

## Progress
- ✅ **Item 1 (real login)** — commit 0bcc95c19. `LoginForm` + `AuthAction::
  SignIn`/`SignUp` + `run_credential_sign_in`; wired into the mobile account
  sheet; dev picker now debug-only; architect-ui `Input` gained `input_type`.
- ✅ **Item 2 (native token persistence)** — commit 2e193f1b4. `FileTokenStore`
  under `$XDG_DATA_HOME/task/ui-tokens/`.
- ✅ **Item 3 (multi-server)** — commit 06ce03699. `vox_session` ActiveServer
  holder + `vox_url()` reads it; `caller_for` cache keyed by URL; registry
  native persistence + active selection; `ServersPanel` (add by URL / select /
  remove) in the mobile account sheet; app root seeds/syncs the holder + re-runs
  org discovery on switch. *Follow-ups:* surface `ServersPanel` on desktop; write
  the token back into the active `ServerEntry` + per-server re-auth on switch.
- ✅ **Item 4 (watch bridge session tokens)** — commit 5cd2007e2. Bridge accepts
  a real `current_session`-validated token OR the static device token.
- ✅ **Item 5 (env auth secret)** — commit 5cd2007e2. `TASK_AUTH_SECRET`.
- ⏳ **Item 6 (WCSession watch inheritance)** — NEXT. Needs native Swift in the
  dx iPhone app (none today) + device testing; see below.
- ⚠️ **Crate-name gotcha**: the app shell UI crate is **`ui`** (`crates/task/ui`,
  what apps/task/{mobile,desktop,web} depend on), NOT `task-ui`
  (`features/task/task/task-ui`, a separate task-list crate). Verify auth/shell
  changes with `cargo check -p ui` (native) + `cargo check -p task-app-web
  --target wasm32-unknown-unknown` (wasm) — `-p ui --target wasm32` fails on
  getrandom's `wasm_js` flag (only the app crate sets it), and `-p task-ui` is
  the wrong crate entirely.

## Work items

### 1. Real login (client) — `crates/task/ui/src/auth.rs` ✅ done
- Add a `LoginForm` (email + password) + optional `SignUpForm`
  (`sign_up_email_password`). Follow architect-ui primitives (AGENTS.md).
- Add a credential-carrying `AuthAction::SignIn { email, password }`; drop the
  `DEV_ACCOUNTS` password lookup in `resolve_session`. Keep the dev picker behind
  a debug/dev cfg as a shortcut.
- *Missing (build):* the forms + the action variant. Everything else stays.

### 2. Native token persistence — `auth.rs` non-wasm branch
- Wire `FileTokenStore` into the `#[cfg(not(target_arch="wasm32"))]` stubs
  (`load_cached_token`/`save_cached_token`) so desktop/mobile stay logged in.
- *Unwired, not missing.*

### 3. Multi-server — `server_registry.rs` + `vox_session.rs` + `vox_clients.rs`
- Provide `ServerRegistry` + an `active_server: Signal<Option<Uuid>>` at app root.
- "Servers" UI page: add/edit/remove by URL + label; select active; sign in per
  server (writes `session_token`/`my_user_id`/`my_email` into the entry).
- Injection points:
  - `vox_session.rs:29 vox_url()` — read active `ServerEntry.server_url` instead
    of `TASK_VOX_URL`/same-origin (keep env as fallback/default).
  - `vox_clients.rs org_ws_url(slug)` + `caller_for` — re-key the socket cache by
    `(server_url, slug)`; attach `AuthClientMiddleware::bearer(entry.session_token)`.
  - `establish_for<C>` — same bearer injection (all typed clients funnel here).
- Native registry persistence: implement `server_registry.rs` non-wasm
  `save/load_from_storage` (a `servers.json`; reuse the FileTokenStore idiom).
- *Unwired (plumbing); missing: the Servers UI page + `AuthCtx` gaining a server
  dimension (today it's hardwired to the home org / per-email localStorage keys).*

### 4. Watch bridge → real session tokens — `apps/task/server/src/watch_bridge.rs`
- Replace the static `TASK_WATCH_TOKEN` + synthetic `owner_id` with:
  `org.auth.auth.current_session(CurrentSession { token }).await` → `bundle.user.id`.
  (The engine handle is already on `OrgAppState.auth.auth`.) Keep the static token
  as an optional fallback for headless/testing.
- *Unwired, not missing (one call).*

### 5. Server auth secret (deploy) — `apps/task/server/src/lib.rs:1064`
- `DEFAULT_AUTH_SECRET` is a hardcoded dev secret that signs every session token
  (forgeable). Read a per-server secret from env (e.g. `TASK_AUTH_SECRET`) for
  real deployments. Security prerequisite for real accounts.

### 6. Watch inherits iPhone config — WCSession (the capstone)
- The dx iPhone app has **no native code** and persists nothing natively. Add:
  - a native persistence of the active `{server_url, org_slug, session_token}`
    into iOS UserDefaults / an App Group (so native Swift can read it);
  - a native `AppDelegate`/shim in the dx iOS build hosting `WCSession`, pushing
    that config via `updateApplicationContext`;
  - watch: `WCSessionDelegate` receives it into `TaskStore` (drop manual Settings,
    keep as fallback).
- *Missing: the native iOS shim + App-Group persistence. The watch already models
  the exact payload (`TaskServerConfig`).*

### 7. (Later) Server-side token enforcement
- Only `AuthService` is auth-wrapped today; other services carry `user_id`
  in-band. Real login gives a trustworthy id but the server doesn't enforce it on
  task/vault/timer yet. Adding `AuthServerMiddleware` + session checks there is a
  separate, larger hardening item.

## Suggested order
1 (login) + 2 (native persist) → 3 (multi-server) → 5 (auth secret) + 4 (watch
bridge session tokens) → 6 (WCSession inheritance) → 7 (enforcement, later).

Items 1–3 are the user-facing unlock ("add task.starcommand.live, log in, stay
logged in"). 4–6 make the watch ride real auth + inherit the phone's server.
