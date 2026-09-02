#!/usr/bin/env bash
# demo.sh — the example studio, running: two companies, two servers, two
# iroh endpoints, federating over the wire.
#
# This is `examples/studio` — the same tree the integration suite reads
# and the same four people it hires — planted on two real data roots and
# served by two real `task-server` processes. Not a script that prints
# `ok`: the thing itself, with a browser pointed at it.
#
# Why two processes rather than one with two orgs: the product is two
# companies on two machines sharing one project. Federation between two
# orgs inside one process is federation with the interesting part taken
# out, and both federation bugs this repo has actually had were of
# exactly that shape.
#
#   ./demo.sh plant    # build + plant both orgs ($DEMO_ROOT/{acme,vnt});
#                      # serve/web/desktop/ids all plant on first use too
#   ./demo.sh fresh    # wipe $DEMO_ROOT, then plant
#   ./demo.sh serve    # run both servers (foreground, Ctrl-C stops both)
#   ./demo.sh web      # run the web app against ACME
#   ./demo.sh desktop  # run the desktop app against ACME
#   ./demo.sh daemon   # a laptop: the sync agent, replicating ACME's projects
#   ./demo.sh ids      # print each org's endpoint id
#   ./demo.sh telemetry       # local OTLP backend (Grafana/Tempo/Loki/Prom)
#   ./demo.sh telemetry-stop  # stop it (data survives; rm to wipe)
#
# Two terminals: `serve` in one, `web` (or `desktop`) in the other.
#
# Everything lives under $DEMO_ROOT (default ~/.local/share/task-demo) —
# throwaway, never prod. Env: TASK_DEMO_ROOT, ACME_PORT (18080), VNT_PORT
# (9102), WEB_PORT (8766).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DEMO_ROOT="${TASK_DEMO_ROOT:-$HOME/.local/share/task-demo}"
# ACME's default is 18080 — the repo's ONE canonical local-server port:
# `DEFAULT_VOX_URL` points there, and `apps/web/Dioxus.toml`'s committed
# dev proxy forwards `/org` + `/media` + `/blobs` there, which is what
# lets the web app's same-origin media fetches (the song player!) reach
# the server under `dx serve`. A second port would need a second proxy
# config, and dx reads only the committed one.
ACME_PORT="${ACME_PORT:-18080}"
VNT_PORT="${VNT_PORT:-9102}"
WEB_PORT="${WEB_PORT:-8766}"

# Both servers write their own endpoint address here and read each
# other's out of it. It stands in for discovery: n0's DNS is what a
# deployment uses and it needs the internet, which a demo on a laptop in
# a basement does not have. Everything above it still dials by bare
# endpoint id — see apps/server/src/iroh_host.rs.
PEERS="$DEMO_ROOT/peers"

# The cast the app's pickers offer, and the account it boots signed in
# as (the first entry — Alice). Format `email:password:Name:username`,
# comma-separated; read by debug builds only (crates/ui/src/auth.rs,
# `TASK_DEMO_CAST`). ACME's server seeds exactly these three — Victor
# lives on VNT's, so he is not in a roster pointed at ACME.
PASSWORD="correct-horse-battery-staple"

# ── local telemetry ──────────────────────────────────────────────────
# The server and the desktop app already export OTLP traces, logs and
# metrics whenever OTEL_EXPORTER_OTLP_ENDPOINT is set, and stay silent
# when it is not (architect-telemetry; the cluster sets it, local runs
# don't). `telemetry` runs grafana/otel-lgtm — Grafana + Tempo + Loki +
# Prometheus in one container, OTLP in on 4318 — and `serve`/`desktop`
# attach to it AUTOMATICALLY whenever something is listening there, so
# the order of `telemetry` vs `serve` is the only thing to remember:
# collector first, then the processes you want traced.
OTLP_PORT="${OTLP_PORT:-4318}"
GRAFANA_PORT="${GRAFANA_PORT:-3000}"
TELEMETRY_NAME="task-demo-telemetry"

telemetry() {
  if docker ps --format '{{.Names}}' | grep -qx "$TELEMETRY_NAME"; then
    echo ">> telemetry already running"
  elif docker ps -a --format '{{.Names}}' | grep -qx "$TELEMETRY_NAME"; then
    docker start "$TELEMETRY_NAME" >/dev/null
    echo ">> telemetry restarted (existing container, data kept)"
  else
    docker run -d --name "$TELEMETRY_NAME" \
      -p "127.0.0.1:$GRAFANA_PORT:3000" \
      -p "127.0.0.1:4317:4317" \
      -p "127.0.0.1:$OTLP_PORT:4318" \
      grafana/otel-lgtm:latest >/dev/null
    echo ">> telemetry started (grafana/otel-lgtm)"
  fi
  echo ">> Grafana   http://127.0.0.1:$GRAFANA_PORT   (anonymous admin; Drilldown → Traces/Logs/Metrics)"
  echo ">> OTLP in   http://127.0.0.1:$OTLP_PORT      (http/protobuf; 4317 grpc)"
  echo ">> servers/apps started AFTER this attach automatically — restart '$0 serve' if it was already up"
}

telemetry_stop() {
  docker stop "$TELEMETRY_NAME" >/dev/null 2>&1 && echo ">> telemetry stopped" \
    || echo ">> telemetry was not running"
}

# Export OTEL_EXPORTER_OTLP_ENDPOINT when a collector is listening.
# Detection over configuration: the exporter retries an unreachable
# endpoint noisily for the life of the process, so pointing at a
# collector that is not there is worse than staying silent. An endpoint
# already in the environment always wins.
telemetry_env() {
  if [ -n "${OTEL_EXPORTER_OTLP_ENDPOINT:-}" ]; then
    echo ">> telemetry: exporting to $OTEL_EXPORTER_OTLP_ENDPOINT (from env)"
  elif (exec 3<>"/dev/tcp/127.0.0.1/$OTLP_PORT") 2>/dev/null; then
    exec 3>&- 2>/dev/null || true
    export OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:$OTLP_PORT"
    echo ">> telemetry: collector on :$OTLP_PORT — exporting traces/logs/metrics"
  fi
}
DEMO_CAST="alice@acme.test:$PASSWORD:Alice:alice,sam@acme.test:$PASSWORD:Sam:sam,casey@client.test:$PASSWORD:Casey:casey"

plant() {
  echo ">> planting the example studio → $DEMO_ROOT"
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DEMO_ROOT/acme" \
      cargo run --quiet -p task-server -- admin demo --org acme-audio )
  echo
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DEMO_ROOT/vnt" \
      cargo run --quiet -p task-server -- admin demo --org vnt-video )
  echo
  # Alice's own org, on ACME's data root — two orgs on one server, and
  # the only place a *personal* wiki exists (Bible Study, Cooking).
  # Without it the subscription demo has one org to subscribe to and
  # nothing personal to subscribe from.
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DEMO_ROOT/acme" \
      cargo run --quiet -p task-server -- admin demo --org alice-personal )
  echo
  echo ">> planted. next: '$0 serve' in one terminal, '$0 web' in another."
}

fresh() {
  echo ">> wiping $DEMO_ROOT"
  rm -rf "$DEMO_ROOT"
  plant
}

# Plant on first use rather than refusing: `serve`, `web` and `desktop`
# are the commands people actually reach for, and "run plant first" is a
# step the script can simply take itself. Planting is idempotent, so an
# already-planted root costs a couple of no-op `admin demo` runs.
require_planted() {
  if [ ! -d "$DEMO_ROOT/acme/orgs" ] || [ ! -d "$DEMO_ROOT/vnt/orgs" ]; then
    echo ">> $DEMO_ROOT not planted yet — planting first"
    plant
  fi
}

# An org's endpoint id, once it has bound one. Written by the server on
# first boot and stable across restarts (the key beside it is what makes
# it stable), so this is empty only before the very first `serve`.
id_of() {
  local root="$1" slug="$2"
  cat "$DEMO_ROOT/$root/orgs/$slug/iroh-endpoint-id" 2>/dev/null || true
}

ids() {
  require_planted
  local acme vnt
  acme="$(id_of acme acme-audio)"
  vnt="$(id_of vnt vnt-video)"
  if [ -z "$acme" ] && [ -z "$vnt" ]; then
    echo "no endpoint ids yet — run '$0 serve' once; the servers mint them on first boot"
    return
  fi
  echo "ACME Audio  ${acme:-(not bound yet)}"
  echo "VNT Video   ${vnt:-(not bound yet)}"
  echo
  echo "That id is the whole address. Paste one into the other org to"
  echo "admit it as a host, or into a device to register it — no host,"
  echo "no port, no certificate."
}

serve() {
  # Always plant, not just when the root is missing: planting is
  # idempotent (existing files and accounts are left alone), and it is
  # how seeder UPGRADES — a newly declared project, a new cast member —
  # reach a root planted before they existed. `require_planted`'s
  # existence check is only right for the commands that shouldn't build
  # the server just to look at something (`ids`).
  plant
  telemetry_env
  mkdir -p "$PEERS"
  cd "$REPO_ROOT"
  # Kill both when this shell exits, so Ctrl-C does not leave one
  # server holding a port and an endpoint id.
  trap 'kill 0' EXIT
  echo ">> ACME Audio  http://127.0.0.1:$ACME_PORT   ($DEMO_ROOT/acme)"
  echo ">> VNT Video   http://127.0.0.1:$VNT_PORT   ($DEMO_ROOT/vnt)"
  echo ">> endpoint ids are logged as each org binds; '$0 ids' prints them"
  echo
  TASK_DATA_ROOT="$DEMO_ROOT/acme" TASK_SERVER_BIND="127.0.0.1:$ACME_PORT" \
    TASK_IROH_PEER_DIR="$PEERS" \
    cargo run --quiet -p task-server 2>&1 | sed 's/^/[acme] /' &
  TASK_DATA_ROOT="$DEMO_ROOT/vnt" TASK_SERVER_BIND="127.0.0.1:$VNT_PORT" \
    TASK_IROH_PEER_DIR="$PEERS" \
    cargo run --quiet -p task-server 2>&1 | sed 's/^/[vnt]  /' &
  wait
}

# A third machine on the demo desk: the sync agent, holding replicas of
# ACME's projects the way a laptop would.
#
# The two admissions are done for you, because the point of the demo is
# what syncing looks like and not what pasting an id feels like. On a
# real machine the same two steps are: `fts-files-daemon id` on the
# device, that line into `orgs/<slug>/admitted-devices` on the server.
#
# The peer directory is shared with `serve`, which is what makes bare-id
# dialling work on a desk with no internet.
daemon() {
  require_planted
  local acme device_data device_id admitted
  acme="$(id_of acme acme-audio)"
  if [ -z "$acme" ]; then
    echo "ACME has not bound an endpoint yet — run '$0 serve' once first" >&2
    exit 1
  fi

  device_data="$DEMO_ROOT/device"
  mkdir -p "$device_data" "$PEERS"
  cd "$REPO_ROOT"
  cargo build --quiet -p files-daemon --features daemon-bin --bin fts-files-daemon

  # The device's own id, minted from its data dir before it starts, so
  # the server can be told about it first — an agent whose first dial is
  # refused would just retry, but the demo should not need a second run.
  device_id="$(FTS_FILES_DAEMON_DATA="$device_data" ./target/debug/fts-files-daemon id)"
  admitted="$DEMO_ROOT/acme/orgs/acme-audio/admitted-devices"
  if ! grep -qs "^$device_id\$" "$admitted"; then
    echo "$device_id" >> "$admitted"
  fi

  echo ">> device      $device_id"
  echo ">> coordinator $acme (ACME Audio)"
  echo ">> replicas    $device_data/roots"
  echo ">> admitted in $admitted — the server sweeps it every minute"
  echo
  TASK_IROH_PEER_DIR="$PEERS" \
    FTS_FILES_DAEMON_DATA="$device_data" \
    FTS_FILES_DAEMON_ROOTS="$device_data/roots" \
    FTS_FILES_DAEMON_COORDINATOR="$acme" \
    FTS_FILES_DAEMON_INTERVAL_SECS="${DEMO_SYNC_SECS:-15}" \
    RUST_LOG="${RUST_LOG:-info}" \
    ./target/debug/fts-files-daemon 2>&1 | sed 's/^/[device] /'
}

desktop() {
  require_planted
  telemetry_env
  echo ">> desktop app → ACME on 127.0.0.1:$ACME_PORT"
  echo ">> boots signed in as Alice; sign out to pick Sam or Casey"
  # `TASK_VOX_URL` is the native runtime knob (see
  # crates/ui-core/src/vox_session.rs) — but a server previously chosen
  # in the app's Servers panel is persisted and would silently win over
  # it, so a demo session against a prod entry is a real footgun. Say so
  # rather than guard against it: the registry is the user's.
  echo ">> note: a server selected in the app's Servers panel overrides this URL"
  # The stylesheet is generated and git-ignored (`just desktop-css`) —
  # regenerate when the tool is here, so a fresh clone works.
  if command -v tailwindcss >/dev/null 2>&1; then
    ( cd "$REPO_ROOT" && tailwindcss -i apps/tailwind.css -o apps/desktop/assets/tailwind.css )
  elif [ ! -f "$REPO_ROOT/apps/desktop/assets/tailwind.css" ]; then
    echo "!! no tailwindcss on PATH and no generated stylesheet — run 'just desktop-css' first" >&2
    exit 1
  fi
  cd "$REPO_ROOT/apps/desktop"
  # TASK_IROH_PEER_DIR: the desktop app dials the org over IROH once
  # discovery hands it the endpoint id (crates/ui-core/src/iroh_transport.rs);
  # this is how it resolves the id to an address with no internet — the
  # same directory the two servers exchange addresses through. The
  # TASK_VOX_URL stays as the discovery base and the fallback transport.
  mkdir -p "$PEERS"
  exec env TASK_VOX_URL="ws://127.0.0.1:$ACME_PORT/vox" \
    TASK_DEMO_CAST="$DEMO_CAST" \
    TASK_IROH_PEER_DIR="$PEERS" \
    dx serve --platform desktop --hot-patch false
}

web() {
  require_planted
  echo ">> web app on http://127.0.0.1:$WEB_PORT → ACME on 127.0.0.1:$ACME_PORT"
  echo ">> boots signed in as Alice; sign out to pick Sam or Casey"
  cd "$REPO_ROOT/apps/web"
  # No hot-patching: dx serve's default breaks the wasm build on edits.
  # The base URL. The app appends `/org/<slug>` itself.
  # `TASK_DEMO_CAST` is baked at build time on wasm (`option_env!`),
  # which `dx serve` is — same as TASK_VOX_URL_WEB.
  exec env TASK_VOX_URL_WEB="ws://127.0.0.1:$ACME_PORT/vox" \
    TASK_DEMO_CAST="$DEMO_CAST" \
    dx serve --web --addr 127.0.0.1 --port "$WEB_PORT" --hot-patch false
}

[ $# -eq 0 ] && { echo "usage: $0 plant|fresh|serve|web|desktop|daemon|ids|telemetry|telemetry-stop"; exit 1; }
for cmd in "$@"; do
  case "$cmd" in
    plant) plant ;;
    fresh) fresh ;;
    serve) serve ;;
    web)   web ;;
    desktop) desktop ;;
    daemon) daemon ;;
    ids)   ids ;;
    telemetry) telemetry ;;
    telemetry-stop) telemetry_stop ;;
    *) echo "unknown: $cmd (want plant|fresh|serve|web|desktop|daemon|ids|telemetry|telemetry-stop)" >&2; exit 1 ;;
  esac
done
