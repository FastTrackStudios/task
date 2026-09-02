# Deploying Task

Everything you need to self-host lives in this directory:

```
deploy/
├── README.md            <- you are here
├── docker-compose.yml   <- smallest real deployment (server + web + edge proxy)
└── chart/
    ├── task/                Helm 3 chart (generic defaults)
    ├── values-dev.yaml      EXAMPLE: the starcommand dev env
    └── values-prod.yaml     EXAMPLE: the starcommand prod env
```

The container images are **pure Nix** — there are no Dockerfiles. Each
image is a `dockerTools.streamLayeredImage` defined in the repo-root
`flake.nix` (`task-server-image`, `task-web-image`). The web bundle is
itself a Nix derivation (`task-webapp`'s `dx build`), so the images
self-contain their content with no out-of-band bundle step.

Prebuilt images (pushed by `.forgejo/workflows/images.yml` via skopeo to
the in-cluster LAN registry):

| image | tags |
|---|---|
| `registry.starcommand.live:30050/task-server` | `latest` (main), `dev`, `sha-<short>` |
| `registry.starcommand.live:30050/task-web`    | same |

## The one architectural rule: same origin

The web app is a wasm bundle. When it is built **without**
`TASK_VOX_URL_WEB` baked in (which is how CI builds it), it derives its
websocket/API URL from `window.location` at runtime — `https://x.example.com`
in the address bar means it dials `wss://x.example.com/vox`. That makes the
images env-agnostic: one `task-web` image serves any hostname, no rebuild per
environment. (Local dev keeps the `127.0.0.1:18080` default.)

The price: whatever serves the static bundle must route the API path
prefixes to `task-server` **on the same host**:

| prefix | what lives there |
|---|---|
| `/vox` | legacy/default org websocket |
| `/org` | per-org endpoints: `/org/<slug>/vox`, health, forge webhooks |
| `/server` | server-management vox (org creation/bootstrap) |
| `/blobs` | signed attachment upload/download |
| `/.well-known` | `task-server.json` federation/health document |

Both the compose file's `edge` proxy and the chart's Ingress implement
exactly this split.

## Quickstart: docker compose

```sh
docker compose -f deploy/docker-compose.yml up -d
# open http://localhost:8090
```

That starts `task-server` (state in the `task-data` volume), `task-web`
(static bundle), and an nginx `edge` that does the same-origin split.

First run: a fresh server hosts **zero orgs** ( `/vox` answers 503 until one
exists). The server-management endpoint accepts an unauthenticated
`create_org` only while the data root is empty — create your first org with
the CLI:

```sh
task org create <slug> --home   # pointed at your instance
```

## Quickstart: Helm

```sh
helm install task ./deploy/chart/task \
  --namespace task --create-namespace \
  --set ingress.host=tasks.example.com
```

or with a values file (start from `deploy/chart/values-prod.yaml` — it is an
example for our infra, the chart defaults are generic):

```sh
helm install task ./deploy/chart/task -f my-values.yaml
```

What you get: server Deployment (PVC at `/data`, probes on
`/.well-known/task-server.json`, `Recreate` strategy — single-writer
sqlite), web Deployment, and one Ingress doing the same-origin split.

Recommended production extras:

```sh
kubectl -n task create secret generic task-env \
  --from-literal=TASK_SERVER_SEALING_KEY=$(openssl rand -hex 32)
# then: server.existingSecret: task-env
```

TLS: terminate wherever you already do (external proxy or cert-manager via
`ingress.annotations` + `ingress.tls`). WebSockets pass through Traefik with
no special annotation.

On starcommand specifically, ArgoCD watches this repo: `dev` branch +
`values-dev.yaml` → `tasks-dev.starcommand.live`, `main` + `values-prod.yaml` →
`tasks.starcommand.live`.

## Building the images yourself

The images are Nix derivations — no Docker daemon required to build them.
Every formerly-sibling crate (architect / Editor / architect-ui / daw /
input_actions / keyflow) is a rev-pinned git dependency that cargo (under
crane) fetches itself, so the build needs network access for git deps.

Each `streamLayeredImage` builds an executable that **streams a
docker-archive tarball to stdout**. Pipe it wherever you want — into
`docker load`, `podman load`, or `skopeo copy`:

```sh
# Build + load locally:
$(nix build --no-link --print-out-paths .#task-server-image) | docker load
$(nix build --no-link --print-out-paths .#task-web-image)    | docker load

# Build + push to the LAN registry (what CI does, sans daemon):
$(nix build --no-link --print-out-paths .#task-web-image) \
  | nix run nixpkgs#skopeo -- --insecure-policy copy --dest-tls-verify=false \
      docker-archive:/dev/stdin \
      docker://registry.starcommand.live:30050/task-web:dev
```

The web bundle is built **without** `TASK_VOX_URL_WEB` baked in (the
`task-webapp` derivation in `flake.nix`), so it stays env-agnostic and
one image serves any hostname. Both images serve / listen on port
**8080**.

## Env var reference (task-server)

Canonical list: `.env.example` + `apps/server/src/main.rs`. The image bakes
sane defaults (`TASK_DATA_ROOT=/data`, `TASK_SERVER_BIND=0.0.0.0:8080`).

| var | meaning |
|---|---|
| `TASK_DATA_ROOT` | **the** state directory — orgs (sqlites, vault, crdt, wiki), attachment blobs, server keypair |
| `TASK_SERVER_BIND` | listen address |
| `TASK_SERVER_ORG` | host only this org slug (unset = serve all under `orgs/`) |
| `TASK_SERVER_PUBLIC_URL` | public base URL used to mint signed attachment URLs — set it behind any proxy (chart derives `https://<ingress.host>`) |
| `TASK_SERVER_SEALING_KEY` | hex key (`openssl rand -hex 32`) sealing at-rest webhook secrets / integration tokens; unset = dev fallback key + warning |
| `TASK_FORGEJO_BASE_URL` / `TASK_FORGEJO_TOKEN` / `TASK_FORGEJO_BOT_TOKEN` / `TASK_FORGE_POLL_SECS` | forge sync (optional) |
| `HERMES_BASE_URL` / `HERMES_SESSION_TOKEN` / `HERMES_DEFAULT_BOARD` / `HERMES_DEFAULT_PROFILE` / `HERMES_WEBHOOK_SECRET` | hermes agent-dispatch integration (optional) |
| `TASK_SERVER_VAULT_ROOT` / `TASK_SERVER_CRDT_ROOT` / `TASK_SERVER_WIKI_ROOT` / `TASK_SERVER_MAIL_ROOT` | per-area path overrides; default under the data root — leave alone in containers |
| `RUST_LOG` | tracing filter (default `info` in the image) |
| `TASK_SENTRY_DSN` | Sentry error reporting (optional; unset = no reporting) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | **opt-in** OpenTelemetry export — traces, logs, and metrics over OTLP `http/protobuf` (e.g. `http://otel-collector-opentelemetry-collector.observability.svc:4318`). Unset = the server emits no telemetry at all |
| `OTEL_EXPORTER_OTLP_PROTOCOL` / `OTEL_EXPORTER_OTLP_HEADERS` / `OTEL_RESOURCE_ATTRIBUTES` | standard OTLP knobs, honoured by the SDK when the endpoint is set |
| `TASK_TELEMETRY_TEMPO_URL` | **opt-in** read access to traces: the Tempo HTTP API base (e.g. `http://tempo.observability.svc:3200`). Enables `telemetry_query_traces` / `telemetry_get_trace` on the account MCP lane (`POST /mcp`), operator-only (static `TASK_MCP_TOKEN` or a home-org `admin`). Unset = those tools are not listed |
| `TASK_TELEMETRY_LOKI_URL` | **opt-in** read access to logs: the Loki HTTP API base (e.g. `http://loki.observability.svc:3100`). Enables `telemetry_query_logs`, same gate. See `docs/observability.md` |

`TASK_VOX_URL` / `TASK_VOX_URL_WEB` are **client-side** knobs (CLI/desktop
runtime env, wasm compile-time bake) — not server config, and deliberately
absent from CI web builds.

## Backups

`TASK_DATA_ROOT` (the `/data` volume / the chart's PVC) is everything:
org sqlites, vault markdown, crdt docs, wiki, attachment blobs,
`server-key.ed25519`. Snapshot that directory and you can rebuild the
instance from scratch; lose it and it's gone. The chart marks the PVC
`helm.sh/resource-policy: keep` so `helm uninstall` does not take your data
with it.

### Server-native git snapshots

The server snapshots itself — `backup.git.*` in the chart turns it on.
A snapshot cycle (the `org-snapshot` engine inside task-server):

1. **quiesce** — a global write gate parks new vox requests at dispatch
   entry;
2. **checkpoint** — `PRAGMA wal_checkpoint(TRUNCATE)` on every open
   sqlite pool (the server runs its sqlites in WAL mode), so each main
   `.sqlite` file is complete and **consistent** — this is the
   consistency story the old CronJob script could only approximate with
   live file copies;
3. **commit** — detached git dirs under `/data/.gitstate/`: one repo per
   org (`orgs/<slug>` → `<orgRepoPrefix><slug>`, continuing any
   pre-existing vault history — repos are never re-inited) plus a
   full-state repo (`/data` minus `.gitstate/`, `lost+found/`, sqlite
   WAL sidecars → `fullRepo`); stray embedded `.git` dirs are
   de-gitlinked, clean trees skip the commit;
4. **release + push** — the gate reopens before the network part; remote
   repos are auto-created (private) via the Forgejo API on first push,
   pushes are plain fast-forwards, never `--force`.

The chart's CronJob is now just a scheduler: it POSTs
`/server/snapshot` on the server service with
`Authorization: Bearer <GIT_TOKEN>` (the same `existingSecret` the
server gets as `TASK_BACKUP_GIT_TOKEN`). In-cluster it talks straight
to the Service, so the synchronous cycle (which can outrun a public
proxy's request timeout) completes fine. If the running image predates
the endpoint the job fails with a clear upgrade hint instead of
silently doing nothing.

**Triggering from outside the cluster** (e.g. a manual snapshot via the
public ingress): a full cycle — quiesce, checkpoint, commit, push every
repo — can take longer than a CDN/proxy request timeout (Cloudflare cuts
at ~100s), so the synchronous `POST /server/snapshot` returns a gateway
error even though the snapshot succeeds. Use the **async kick-off**
instead:

```sh
# fire it off — returns 202 immediately
curl -X POST -H "Authorization: Bearer $TOKEN" \
  "https://tasks.example.com/server/snapshot?wait=0"

# poll until phase is "done" (or "failed")
curl -H "Authorization: Bearer $TOKEN" \
  "https://tasks.example.com/server/snapshot/status"
# → { "phase": "done", "stamp": "...", "repos": [ {repo,committed,pushed,...} ] }
```

`?wait=0` runs the cycle on a background task; `409` if one is already
running. The status endpoint reports the last async cycle's per-repo
results. (`task admin snapshot` over `/server/vox` doesn't need this —
the websocket connection isn't subject to the proxy's request timeout.)

On-demand admin verbs (over `<server>/server/vox`, same connection
style as `task org create`):

```sh
task admin snapshot                 # run a cycle now
task admin log [--limit N]          # recent snapshot commits (full repo)
task admin branch <name>            # branch the data at HEAD (and push)
task admin restore <commit> --yes   # roll the data root back
```

`restore` checks out the commit over `/data` via the full-state repo
(taking a rescue snapshot first unless `--force`), deletes stale sqlite
WAL sidecars, then **exits the server process** — k8s/systemd restart it
on the restored data. Local dev runs must restart `task-server` by
hand. Roll *forward* again by restoring the rescue commit.

**Future direction — storage backends beyond a forge.** The push target
is deliberately one narrow function (`SnapshotEngine::push` in
`features/org/org-snapshot`): today it speaks git-over-https with
Forgejo auto-create, or a plain directory of bare repos (NAS mount).
A Nextcloud / WebDAV / rclone-style remote slots in behind that same
function without touching the snapshot machinery.

A periodic filesystem-level snapshot of the PVC remains a fine
belt-and-braces layer on top.

## Upgrades

Images are interchangeable per tag; data compatibility is governed by
**schema stamps** — `/.well-known/task-server.json` publishes a stamp per
service, and clients refuse to sync against a mismatched stamp. After
upgrading the server image, run

```sh
task doctor
```

against the instance to surface stamp drift / migration needs. Roll back by
pinning the previous `sha-<short>` image tag (data is forward-written —
verify a backup exists before big jumps).

Sequence for k8s: bump the image tag (or let ArgoCD follow `dev`/`main`),
the server pod recreates (`Recreate` strategy frees the PVC first), web pods
roll whenever — they are stateless.
