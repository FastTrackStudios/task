# Observability

How this repo is instrumented, how to add to it, and how to read it back.
For an engineer or an agent about to touch a handler, an RPC, or a bug
report from production.

The doctrine comes from the wide-events (canonical log lines) pattern —
`~/.claude/skills/logging-best-practices/` holds the long form, and its
`rules/fts-rust.md` is the Rust companion this page grew out of.

## 1. The doctrine, one page

**The span is the wide event.** Do not build a `HashMap` of context and
log it at the end; do not sprinkle `info!` lines through a handler. The
request-scoped container already exists, already propagates across
`.await`, and already reaches Tempo:

| lane | who opens the span | what it carries |
|---|---|---|
| vox RPC | `architect`'s `LayerRouter::call_span` — one span per dispatched call, named `Svc/method` via `otel.name` | `rpc.service`, `rpc.method`, `rpc.scope` |
| HTTP | `tower_http::trace::TraceLayer` in `apps/server/src/main.rs` (`http.request`) | `method`, **matched** `route`, `status` |
| MCP (`POST /mcp`, `POST /org/{slug}/mcp`) | the HTTP span above | plus `mcp.tool` once a tool is named |

Rules that follow from it:

- **One event per request per service.** Everything you learn while
  serving the request is a field on that span, not a new line.
- **Enrich, don't scatter.** Add fields at the point you learn them.
  Wrap the seam the framework already calls once per request rather than
  editing the framework (see §2).
- **Allowed outcomes ride the span only.** A log line per allow is the
  scatter the pattern exists to delete.
- **Denials and refusals get one `warn!` line.** They are alertable, and
  the line carries the same trace id as the span, so Loki and Tempo meet
  on it.
- **High cardinality is welcome** (`org.slug`, `auth.user_id`, trace
  ids) — that is what makes "this one user's failing request" a query.
  Unbounded cardinality from raw input (paths, URIs, free text) is not.

## 2. Adding a field

`tracing` macros take a *static* field list. `architect_telemetry::wide`
writes a dynamic attribute onto the current OTel span and is a no-op when
no OTel layer is installed, so library code calls it without gating:

```rust
use architect_telemetry::wide;

wide::set("auth.principal_kind", "anonymous");     // &'static str
wide::set("auth.user_id", user_id.clone());        // String
wide::set_display("perm.resource", &resource);     // anything Display
```

Prefer a **decorator on an existing seam** to a call inside every
handler. The pattern, as practised in `apps/server/src/permits.rs`:

| seam the framework calls per request | Task's wrapper | fields it sets |
|---|---|---|
| `IdentityResolver` (`architect_permissions`) | `permits::AuditedIdentityResolver` around `SessionIdentityResolver` | `auth.principal_kind`, `auth.user_id`, `auth.token_presented`, `auth.outcome`, `org.slug` |
| `IdentityResolver` | `permits::HomeFallbackResolver` (home-org token + membership row) | `auth.cross_org`, `auth.membership_role` |
| `IdentityResolver` | `central_auth::CentralFallbackResolver` (issuer introspection) | `auth.central` |
| `AuditSink` (the permission gate's per-decision hook) | `permits::GateAudit` | `perm.decision`, `perm.mode`, `perm.principal`, `perm.resource`, `perm.action`, `perm.reason`, `perm.default_role` |
| an MCP `tools/call` | `mcp::telemetry_call` | `mcp.tool`, `telemetry.backend`, `telemetry.outcome` |

To add a dimension: find the trait the framework already calls once per
request, wrap it, set fields. If no seam exists, set the field at the one
place in the handler where the fact becomes known — still a field, never
a line.

## 3. Field-name registry

Renaming a field breaks every saved query. Keep these stable; add new
families here when you introduce them.

| Field | Values | Set by |
|---|---|---|
| `rpc.service` / `rpc.method` | e.g. `TimerService` / `list_sessions` | architect |
| `rpc.scope` | instance scope, `""` when unscoped | architect |
| `route` / `method` / `status` | matched HTTP route, verb, status code | `main.rs` TraceLayer |
| `org.slug` | org slug — high cardinality, keep it | `permits` |
| `auth.principal_kind` | `user` \| `anonymous` \| `static_token` | `permits`, `mcp` |
| `auth.user_id` | present when resolved | `permits` |
| `auth.token_presented` | bool | `permits` |
| `auth.outcome` | `resolved` \| `rejected` \| `absent` | `permits` |
| `auth.cross_org` | `member` \| `not_a_member` \| `lookup_failed` \| `unparsable_user_id` | `permits::HomeFallbackResolver` |
| `auth.membership_role` | the row's role, `(member)` when none | `permits::HomeFallbackResolver` |
| `auth.central` | `session_token` \| `access_token` \| `issuer_unreachable` \| `unrecognised` \| `no_token` … | `central_auth` |
| `perm.decision` | `allow` \| `deny` \| `would_deny` | `permits::GateAudit` |
| `perm.mode` | `enforcing` \| `observe-only` | `permits::GateAudit` |
| `perm.principal` / `perm.resource` / `perm.action` / `perm.reason` / `perm.default_role` | | `permits::GateAudit` |
| `media.authorized` / `media.auth_via` / `media.mode` / `media.token_error` | signed-URL media route | `lib.rs` (media route) |
| `dav.auth_via` | WebDAV bridge auth path | `webdav.rs` |
| `share.outcome` / `share.label` / `share.guest` | share-link resolution | `share.rs`, `share_guest.rs` |
| `mcp.tool` | the MCP tool name (`create_task`, `telemetry_query_logs`, …) | `mcp` |
| `telemetry.backend` | `tempo` \| `loki` | `mcp::telemetry_call` |
| `telemetry.outcome` | `ok` \| `refused` \| `unconfigured` \| `bad_request` \| `upstream_error` | `mcp::telemetry_call` |
| `wiki.subscribe.source` / `wiki.subscribe.outcome` | subscription materialisation | `wiki-live` |
| `wiki.slug` | the wiki being served — *being added on `wiki-multi-spec`* | `wiki-live` |
| `wiki.edit.id` / `wiki.edit.outcome` | LLM edit proposal id and its fate — *being added on `wiki-multi-spec`* | `wiki-live` |
| `wiki.source.outcome` / `wiki.source.commit` | source ingest result and the commit it landed as — *being added on `wiki-multi-spec`* | `wiki-live` |

`auth.outcome` is the model example of a field earning its place:
`rejected` (token sent, store said no) and `absent` (no token) are the
*same* `Principal::Anonymous`, and telling them apart is the difference
between "sessions are expiring" and "the UI never signed in".

## 4. Hard rules

| never | instead |
|---|---|
| a token, password, session id, or API key in a field — fields are exported off-box | the *shape*: `auth.token_presented: true`, `auth.outcome: rejected` |
| a raw request URI or a note/vault path — org slugs and note names leak, and the cardinality is unbounded in the bad way | the **matched route** (`/org/{slug}/vox`), and the org as `org.slug` |
| free text from the client as a field value | an enum you control (`outcome: refused`) |
| `println!` / `eprintln!` / `dbg!` in server code — they bypass the subscriber and cannot be filtered | a field, or a structured `tracing::debug!` you delete (§5) |
| an `info!` per successful request | nothing — the span already exists |

Bounded cardinality means: the *set of keys* is fixed by the code, and
each value is either something we mint (ids, slugs) or a closed enum.

## 5. Debugging is not printing

The `eprintln!` reflex applies to scaffolding too, and the order is:

1. **Reproduce in a failing test.** A test outlives the session, pins
   the bug, proves the fix, guards the regression. Print output proves
   nothing once the terminal scrolls. In tests, assertions with context
   (`assert!(cond, "org {slug}: {value:?}")`) replace prints.
2. **Query the span.** If the code runs under a request span the answer
   is usually one missing `wide::set` away. Add the field, keep it, read
   it in Tempo (§6) — it is useful forever.
3. **Live process, last resort.** `tracing::debug!` with *structured*
   fields behind `RUST_LOG=task_server::module=debug`, and delete it
   before committing.

## 6. Querying

### Through MCP — the `telemetry_*` tools

On the account lane (`POST /mcp`, `apps/server/src/mcp.rs`), when the
server has `TASK_TELEMETRY_TEMPO_URL` and/or `TASK_TELEMETRY_LOKI_URL`
set. Operator-only: the static `TASK_MCP_TOKEN`, or a session holding
`admin` in the home org. Backed by `apps/server/src/telemetry_query.rs`.

| tool | arguments | returns |
|---|---|---|
| `telemetry_status` | — | `{backends: {tempo, loki}, allowed}` |
| `telemetry_query_traces` | `traceql`, `since?` (`15m`/`2h`/`1d`, default `1h`), `limit?` (≤200, default 20) | `traces: [{trace_id, root_service, root_name, start, duration_ms, span_count}]` |
| `telemetry_get_trace` | `trace_id` | `spans: [{span_id, parent, service, name, start, duration_ms, status, attributes}]` sorted by start, attributes flattened to `key: value` |
| `telemetry_query_logs` | `logql`, `since?`, `limit?` | `logs: [{ts, labels, line}]` newest first, ANSI stripped |

Every result carries `count` and `truncated` (≈48 KB cap). The loop is:
`telemetry_query_traces` to find the request → `telemetry_get_trace` to
read its wide event → `telemetry_query_logs` only for the one-line
refusals, boot lines and panics that are *not* on a span.

### TraceQL cookbook (Tempo)

Span attributes are addressed as `span.<field>`, resource attributes as
`resource.<field>`. Prod's service is `task-server`.

```
# sessions being refused — tokens sent, store said no
{ resource.service.name = "task-server" && span.auth.outcome = "rejected" }

# clients that never sent a token at all (the UI is not signed in)
{ resource.service.name = "task-server" && span.auth.outcome = "absent" }

# permission denials in one org
{ span.perm.decision = "deny" && span.org.slug = "fasttrackstudios" }

# what enforcement WOULD deny before flipping TASK_ENFORCE_PERMISSIONS
{ span.perm.decision = "would_deny" } | count() by (span.rpc.service, span.rpc.method)

# slow RPCs
{ span.rpc.service = "TimerService" && duration > 500ms }

# every call of one method, slowest first (sort in the UI / by duration)
{ span.rpc.method = "list_sessions" && span.org.slug = "codywright" }

# home-org tokens reaching another org without a membership row
{ span.auth.cross_org = "not_a_member" }

# the central issuer being unreachable
{ span.auth.central = "issuer_unreachable" }

# the MCP lane: which tools agents call, and which get refused
{ span.mcp.tool != "" } | count() by (span.mcp.tool)
{ span.telemetry.outcome = "refused" }

# anything that errored
{ resource.service.name = "task-server" && status = error }
```

### LogQL cookbook (Loki)

Labels available on prod: `service_name`, `namespace`, `app`,
`component`, `container`. Only refusals, denials, boot lines and panics
are lines — allowed requests are spans, query those in Tempo.

```
# the one-line warnings around central auth
{service_name="task-server"} |= "central auth"

# denials, by structured field
{service_name="task-server"} | json | perm_decision="deny"

# panics and errors, all containers in the namespace
{namespace="task"} |~ "panic|ERROR"

# a specific trace's log lines (trace id from telemetry_query_traces)
{service_name="task-server"} |= "0af7651916cd43dd8448eb211c80319c"

# boot lines — kubectl logs never reaches them past the iroh noise
{namespace="task", container="task-server"} |= "listening"
```

### Fallback — the in-cluster URLs directly

Use this when the MCP lane itself is what is broken. Grafana is at
`grafana.starcommand.live`; the APIs the tools call are ClusterIP
services, so run curl from a pod. Log lines carry ANSI escapes — strip
with `sed 's/\x1b\[[0-9;]*m//g'`.

```bash
ssh starcommand 'kubectl -n observability run q --rm -i --restart=Never \
  --image=curlimages/curl:latest --quiet -- sh -c \
  "curl -sG http://loki.observability.svc:3100/loki/api/v1/query_range \
   --data-urlencode '\''query={namespace=\"task\"} |~ \"central auth\"'\'' \
   --data-urlencode '\''since=45m'\''"'
```

Same shape for Tempo: `http://tempo.observability.svc:3200/api/search`
with `q=<TraceQL>`, `start`/`end` in unix seconds, `limit`; and
`http://tempo.observability.svc:3200/api/traces/<traceID>` for one trace.

Why not `kubectl logs`: iroh emits `net_report: IPv4 address detected by
QAD varies by destination` several times a second, so a `--tail` window
never reaches anything real. Loki's filters find what a tail cannot.

## 7. Local dev

```bash
just demo telemetry        # grafana/otel-lgtm: Grafana + Tempo + Loki + Prometheus
just demo serve            # started AFTER telemetry, attaches automatically
just demo telemetry-stop   # data survives; `docker rm task-demo-telemetry` wipes it
```

| what | where |
|---|---|
| Grafana (anonymous admin; Drilldown → Traces / Logs / Metrics) | `http://127.0.0.1:3000` |
| OTLP in (http/protobuf) | `http://127.0.0.1:4318` (`4317` grpc) |
| Tempo / Loki APIs (to use the `telemetry_*` tools locally) | not published by default. Add `-p 127.0.0.1:3200:3200 -p 127.0.0.1:3100:3100` to the `docker run` in `scripts/demo.sh` `telemetry()`, then start the server with `TASK_TELEMETRY_TEMPO_URL=http://127.0.0.1:3200 TASK_TELEMETRY_LOKI_URL=http://127.0.0.1:3100` |

`scripts/demo.sh` exports `OTEL_EXPORTER_OTLP_ENDPOINT` only when
something is listening on `:4318` — the exporter retries an unreachable
collector noisily for the life of the process, so detection beats
configuration. Order is the one thing to remember: collector first, then
the processes you want traced.

## Where things live

| | |
|---|---|
| span enrichment API | `architect_telemetry::wide` (`features/telemetry` in the architect repo, `otel` feature) |
| HTTP span + metrics | `apps/server/src/main.rs` (`TraceLayer`, `http_metrics`) |
| auth / permission fields | `apps/server/src/permits.rs`, `apps/server/src/central_auth.rs` |
| telemetry read client | `apps/server/src/telemetry_query.rs` |
| MCP `telemetry_*` tools | `apps/server/src/mcp.rs`, e2e in `apps/server/tests/mcp_telemetry_e2e.rs` |
| prod env (URLs) | `~/.starcommand/modules/services/task/task.nix`; chart notes in `deploy/chart/task/values.yaml`, `deploy/README.md` |
| the stack itself | `~/.starcommand/modules/services/observability/observability.nix` |
