# Observability: OTLP traces, logs, and metrics

**Status:** in progress

Full traceability for the Task backend — one OTLP pipe out of the
server into the cluster's observability stack, so errors and latency
are queryable over time instead of scrolling past in `kubectl logs`.

## Shape

```
task-server ──OTLP/http──▶ otel-collector ──▶ Tempo       (traces)
  (tracing spans,          (observability     ├─▶ Loki    (logs)
   log records,             namespace)        └─▶ Prometheus (metrics)
   RED metrics)                                     │
                                            Grafana ┘ (grafana.starcommand.live)
```

The cluster already runs Prometheus, Grafana (Authelia SSO), Loki, and
Promtail via `FTS.cluster.services.observability` in the starcommand
repo. This effort adds **Tempo** and an **OTel collector** next to
them, and makes the app emit.

## Opt-in, always

Task's product promise is local-first and telemetry-free. Both the
Sentry DSN and the OTLP endpoint follow the same rule: **unset means
nothing is emitted**. The `otel` cargo feature gates the dependency
weight (only `task-server` enables it); `OTEL_EXPORTER_OTLP_ENDPOINT`
gates the runtime. A self-hosted instance that sets neither ships no
data anywhere, ever.

## Done

- `architect-telemetry` grew an `otel` module (feature `otel`): builds OTLP
  span / log / metric exporters from the standard `OTEL_EXPORTER_OTLP_*`
  env vars, returns tracing layers to compose into the registry plus a
  guard that flushes on drop. `enabled()` reports whether the endpoint
  is configured.
- `task-server` composes those layers into its existing registry
  (fmt + Sentry + OTel), so every `tracing` span becomes a trace and
  every event becomes an OTel log record.
- HTTP layer: one span per request (method + **matched route**, never
  the raw URI — org slugs and note paths must not become labels) and
  RED metrics (`http.server.requests`, `http.server.request.duration`).
- Chart: `server.env` documents the OTLP knobs;
  `values-fasttrackstudio.yaml` points at the in-cluster collector.

- **Cluster** (starcommand repo, `modules/services/observability/`) —
  LIVE since 2026-08-06: the OTel collector (`otel-collector`, OTLP on
  4317/4318) and Tempo (`tempo-0`) run in the `observability` namespace
  beside the existing Prometheus/Grafana/Loki/Promtail, Grafana carries
  the Tempo datasource with span↔logs correlation both ways, and
  `grafana.starcommand.live` has its Caddy route.
- **Prod emits** — the ArgoCD Application's inline values
  (`~/.starcommand/modules/services/task/task.nix`) set
  `OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector.observability.svc:4318`.
  NOTE: `values-fasttrackstudio.yaml` in this repo is the SELF-HOSTER
  EXAMPLE and keeps the endpoint commented out; it is not what prod
  runs. Reading it as prod config is a mistake that has been made — go
  to the Application values, or `kubectl exec … env`, for what is live.

## Where to look

Retention: **logs 31d** (Loki), **traces 7d** (Tempo), **metrics 30d**
(Prometheus) — so `kubectl logs` scrolling past is not data loss; the
pod's own log file only covers a couple of minutes at INFO, but Promtail
has already shipped it. Grafana: <https://grafana.starcommand.live>.
Ad-hoc, without leaving the shell:

```sh
# logs for a phrase, last 3h
kubectl exec -n task deploy/task-server -- sh -c \
  "curl -sG --data-urlencode 'query={namespace=\"task\"} |= \"protocol violation\"' \
    --data-urlencode 'since=3h' http://loki.observability.svc:3100/loki/api/v1/query_range"
# recent traces
kubectl exec -n task deploy/task-server -- sh -c \
  "curl -sG --data-urlencode 'q={resource.service.name=\"task-server\"}' \
    http://tempo.observability.svc:3200/api/search"
```

## Remaining
- **ServiceMonitor** for task-server so Prometheus scrapes it directly
  as well (the collector path covers push; a scrape target is better
  for uptime). Needs a metrics port on the deployment.
- **Dashboards**: request rate / error rate / p95 by route, vox RPC
  volume, sync + collab health. Provision as ConfigMaps so they are
  code, not clicked.
- **Alerts**: extend the existing `PrometheusRule` pattern — 5xx rate,
  p95 latency regression, restart loops.
- **Log volume**: the server runs at `RUST_LOG=info`, which emits a line
  per sqlx query and per loro block-encode. Every one of those is now
  also an OTLP log record shipped to Loki. `info,sqlx=warn,loro_internal=warn`
  would cut the bulk without losing a single application event.
- **The engine** (`apps/fasttrackstudio`): `architect::host::init_tracing`
  builds a non-layered subscriber, so it needs restructuring before the
  live rig can export. Low priority — and the rig should probably stay
  local-only anyway.
- **Client-side**: the wasm app has no exporter. Sentry already covers
  crashes; browser OTLP is a bigger call (CORS, PII, volume).
