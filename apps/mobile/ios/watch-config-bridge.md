# iOS → Watch config bridge (WCSession) — implementation guide

**Status: watch-side DONE, iOS-side sender BUILT (pure Rust, see below) —
remaining: on-device acceptance test via TestFlight.** This is subtask **S6**
of the federated-account epic (Task issue `8821acac`).

## Goal

The Apple Watch has no independent way to know which Task server + account to
talk to. It should **inherit the paired iPhone's active config** — the
`{baseURL, orgSlug, token}` the phone is currently signed into (including
whatever the federated-locker flow selected) — instead of the user typing it
into the watch's Settings by hand.

Transport: **WatchConnectivity `updateApplicationContext(_:)`** — a small,
latest-value-wins key/value dictionary that iOS delivers to the watch even when
it's not running. Perfect for config (not a message stream).

## Wire contract (keys MUST match on both sides)

```
["baseURL": String,   // e.g. "https://task.starcommand.live"
 "orgSlug": String,   // e.g. "codywright"
 "token":   String]   // the phone's active session token (or device token)
```

## Watch side — DONE (this PR)

- `apps/task/watchos/TaskWatch/Model/PhoneSync.swift` — a `WCSessionDelegate`
  that activates the session and applies received context to `TaskStore`
  (`baseURL`/`orgSlug`/`token`), only overwriting non-empty values so a partial
  context never wipes a working manual config.
- Wired in `TaskWatchApp.swift` via `.onAppear { phoneSync.start(store: store) }`.

The watch will inherit config the moment an iOS sender exists. Manual Settings
stays as the fallback. (Verify the watch build on airlock — no watchOS SDK in
the Linux dev shell.)

## iOS side — BUILT (pure Rust over objc2; no Swift shim needed)

The "hosting native Swift in the dx app" problem dissolved: the `WCSession`
host is plain Rust via `objc2-watch-connectivity` (crates.io, matches the
tree's objc2 0.6 / objc2-foundation 0.3), linked only on iOS. No Xcode
project surgery, no AppDelegate injection, no `watch-config.json` file —
the trigger lives in Rust where the config actually changes.

Two halves, split on a platform seam:

1. **Observer (shared UI)** — `crates/task/ui/src/watch_sync.rs`.
   `use_watch_config_publisher()` mounts in `App` (after `provide_auth`) and
   recomputes `{baseURL, orgSlug, token}` on every change of the active
   server registry entry, the org selection/discovery, or the signed-in
   account — including boot restore. `baseURL` is derived from the registry's
   vox URL (`wss://host/vox` → `https://host`); the token prefers the live
   session (Guest excluded, mirroring `sync_active_server_entry`) and falls
   back to the entry's persisted token. Incomplete configs are never
   published (matches PhoneSync's non-empty-only application); identical
   repeats are deduped. The config goes to a registered *sink* — with no
   sink (desktop/web/wasm) the whole thing is a no-op.

2. **Sender (iOS shell)** — `apps/task/mobile/src/watch_sync.rs`.
   `watch_sync::init()` (called from `main`, before launch) activates
   `WCSession.default` with a minimal delegate and registers the sink, which
   parks the latest config and calls `updateApplicationContext`. Guarded by
   `isSupported` / activation state / `isWatchAppInstalled`, fail-silent
   (log only); the activation-complete callback replays the parked config,
   and `sessionDidDeactivate` re-activates for a newly-paired watch.
   Everything is `#[cfg(target_os = "ios")]` (deps target-gated in
   `apps/task/mobile/Cargo.toml`); other targets get a no-op `init()`.

Remaining acceptance test (on device): sign in on the phone → within a few
seconds the watch's Settings auto-populate and `Test connection` succeeds,
with no manual entry. Then drop the manual Settings fields to read-only
(inherited) with a manual-override escape hatch.

## Related
- Server bridge that accepts the inherited token: `apps/task/server/src/watch_bridge.rs`
  (`/watch/v1`, accepts a real `current_session`-validated token OR the static
  `TASK_WATCH_TOKEN`).
- Federated-account plan: `apps/task/plans/multi-server-auth.md` item 6.
