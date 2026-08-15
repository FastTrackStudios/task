# Web SSR / fullstack — feasibility & cost/benefit

**Status:** decision record. Conclusion reached (SSR is the wrong first lever); no work queued from it.

Investigation triggered by "the project UI is slow — maybe SSR the web so all
data is ready at all times." Conclusion up front: **SSR is the wrong first
lever for this app, and it's blocked on a prerequisite we don't have today.**
The "data ready" goal is far better served by fixing the data layer (N+1 +
waterfall + a batch RPC). True SSR is a separate, larger project that only
makes sense *after* that.

## How the web app actually works today

- `apps/web` is a **client-only WASM SPA**. `dioxus = { features = ["router"] }`,
  `default = ["web"]` → `dioxus/web` + `dioxus/launch`. `Dioxus.toml`
  `default_platform = "web"`. No `server` / `fullstack` feature anywhere in
  `apps/web` or `crates/ui`.
- **Data path**: browser WASM → opens a **WebSocket per `(service, org)`** via
  `vox_websocket::WsLink::connect(ws_url)` then
  `initiator_on(link, Bare).establish::<Client>()`
  (`crates/ui/src/vox_clients.rs`). Clients are cached for the page lifetime
  because their wasm-bindgen closures must outlive in-flight socket events.
- This data layer is **wasm-only**. Every `establish_for` / `*_client` has a
  `#[cfg(not(target_arch = "wasm32"))]` arm that returns
  `"native client not wired yet"`. `vox_websocket::WsLink` is a *browser*
  WebSocket binding. **There is no native vox client.**
- `apps/server` (`task-server`) is a **separate axum binary** (bind `:9090`),
  multi-org, mounting `/org/<slug>/vox` WebSocket endpoints + REST. It is the
  vox host; it reads SQLite + vault markdown and makes **live codeberg API
  calls** for forge data. The web SPA and task-server are **two distinct
  processes** that talk over WS.

## What Dioxus 0.7 SSR/fullstack requires

- The same component tree compiled twice: wasm (client) + native (server
  feature). Server renders HTML with embedded data; client hydrates.
- Server-side data must flow through `#[server]` fns / `use_server_future` /
  `use_loader` — code that runs **natively on the server binary** and serializes
  results into the HTML.
- Hydration rule: server and client must render identically; all async/
  non-deterministic data must go through those hooks or you get hydration
  mismatch panics.

## Why it's blocked here

1. **No native data path.** To SSR a page, the *server* build
   (`#[cfg(not(wasm32))]`) must fetch the same data the wasm build does. Today
   that's all stubbed. SSR cannot render anything real until a native vox client
   exists. Two ways to get one, both substantial:
   - **(a) Native vox client** — wire `establish_for` over a non-browser
     transport (vox over native ws/tcp) so the SSR server dials task-server
     per org. Adds a hop: browser → dx-ssr-server → task-server → sqlite/codeberg.
   - **(b) Merge servers** — make the dx fullstack server *be* task-server, so
     server-fns hit the org backends in-process (no second hop). Right end
     state, but a large merge: task-server's multi-org router, per-org state,
     and vox mounts must live inside the dx fullstack axum app.
2. **SSR doesn't remove the actual latency — it relocates it.** The slowness is
   live codeberg calls (see below). SSR moves those from browser to server, so
   *time-to-first-byte gets worse* (server now blocks on codeberg before sending
   HTML) unless we *also* fix the N+1 and cache. The wasm bundle still must load
   to hydrate before the page is interactive.
3. **Multi-org "All" mode is client state** (selected slugs in localStorage +
   org switcher). SSR needs the selection at render time → cookies / route
   params. Extra plumbing.
4. **The whole UI is `use_resource` + long-lived cached WS clients** —
   deliberately client-centric to dodge dropped-closure crashes. Converting hot
   pages to `use_server_future` / `use_loader` is a per-page rewrite plus
   hydration-mismatch debugging, not a flag flip.
5. **Auth/secrets**: SOPS forge tokens would now also be needed in the SSR
   render context.

## The real cause of the slow project page (measured from the code)

1. **N+1 live forge calls (dominant).** `forge_views.rs`: `ForgePanel` fetches
   the issue list + PR list, then **every** `IssueManageRow` and **every**
   `PrManageRow` eagerly fires its own `fetch_issue_comments` — one round-trip
   per issue and per PR, each browser → task-server → codeberg. 10 issues +
   5 PRs ≈ **17 live codeberg calls** on open, most for conversations not yet
   visible.
2. **Fetch waterfall.** `project_detail.rs` loads project+tasks, then a
   `use_effect` writes `project_uuid`/`project_slug`, which only *then* triggers
   the threads + repos resources — a second wave gated behind a render cycle
   instead of running concurrently.
3. **Memoization is fine.** The props_lint "missing PartialEq" hits on
   `RepoId` / `IssueState` / `PullRequestState` are **cross-crate false
   positives** — all three derive `PartialEq`. (Real `signal_many_writers`
   findings are in vault/schedule — maintainability, not this page.)

## Recommendation (in order)

1. **Fix N+1 + waterfall** — lazy-load issue/PR comments (fetch on expand) and
   fetch threads/repos concurrently with tasks. Hours of work, removes the
   actual stall, no architecture change. *This is the thing to do first.*
2. **Server-side forge cache** in task-server (issues/PRs/comments, short TTL)
   so codeberg is paid once across views, not per open.
3. **Batch/aggregate RPC** — since the vox WS is already long-lived, add one
   `project_overview(project_id)` RPC that returns project + tasks + threads +
   repos + forge in **one** round-trip instead of ~20. This delivers ~90% of
   "SSR feels instant" with none of the rearchitecture.
4. **Only then**, if still wanted: true SSR, gated on prerequisite (a) or (b)
   above. Scope it as its own project. For a private, logged-in, multi-org,
   WS-backed tool the classic SSR wins (SEO, fast first paint for anonymous
   visitors) mostly don't apply; #1–#3 capture the felt-performance win.
