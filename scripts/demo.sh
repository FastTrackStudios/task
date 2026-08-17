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
#   ./demo.sh plant    # build + plant both orgs ($DEMO_ROOT/{acme,vnt})
#   ./demo.sh fresh    # wipe $DEMO_ROOT, then plant
#   ./demo.sh serve    # run both servers (foreground, Ctrl-C stops both)
#   ./demo.sh web      # run the web app against ACME
#   ./demo.sh ids      # print each org's endpoint id
#
# Two terminals: `serve` in one, `web` in the other.
#
# Everything lives under $DEMO_ROOT (default ~/.local/share/task-demo) —
# throwaway, never prod. Env: TASK_DEMO_ROOT, ACME_PORT (9101), VNT_PORT
# (9102), WEB_PORT (8766).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DEMO_ROOT="${TASK_DEMO_ROOT:-$HOME/.local/share/task-demo}"
ACME_PORT="${ACME_PORT:-9101}"
VNT_PORT="${VNT_PORT:-9102}"
WEB_PORT="${WEB_PORT:-8766}"

# Both servers write their own endpoint address here and read each
# other's out of it. It stands in for discovery: n0's DNS is what a
# deployment uses and it needs the internet, which a demo on a laptop in
# a basement does not have. Everything above it still dials by bare
# endpoint id — see apps/server/src/iroh_host.rs.
PEERS="$DEMO_ROOT/peers"

plant() {
  echo ">> planting the example studio → $DEMO_ROOT"
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DEMO_ROOT/acme" \
      cargo run --quiet -p task-server -- admin demo --org acme-audio )
  echo
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DEMO_ROOT/vnt" \
      cargo run --quiet -p task-server -- admin demo --org vnt-video )
  echo
  echo ">> planted. next: '$0 serve' in one terminal, '$0 web' in another."
}

fresh() {
  echo ">> wiping $DEMO_ROOT"
  rm -rf "$DEMO_ROOT"
  plant
}

require_planted() {
  if [ ! -d "$DEMO_ROOT/acme/orgs" ] || [ ! -d "$DEMO_ROOT/vnt/orgs" ]; then
    echo "!! $DEMO_ROOT not planted yet — run '$0 plant' first" >&2
    exit 1
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
  require_planted
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

web() {
  require_planted
  echo ">> web app on http://127.0.0.1:$WEB_PORT → ACME on 127.0.0.1:$ACME_PORT"
  echo ">> sign in as alice@acme.test / correct-horse-battery-staple"
  cd "$REPO_ROOT/apps/web"
  # No hot-patching: dx serve's default breaks the wasm build on edits.
  # The base URL. The app appends `/org/<slug>` itself.
  exec env TASK_VOX_URL_WEB="ws://127.0.0.1:$ACME_PORT/vox" \
    dx serve --web --addr 127.0.0.1 --port "$WEB_PORT" --hot-patch false
}

[ $# -eq 0 ] && { echo "usage: $0 plant|fresh|serve|web|ids"; exit 1; }
for cmd in "$@"; do
  case "$cmd" in
    plant) plant ;;
    fresh) fresh ;;
    serve) serve ;;
    web)   web ;;
    ids)   ids ;;
    *) echo "unknown: $cmd (want plant|fresh|serve|web|ids)" >&2; exit 1 ;;
  esac
done
