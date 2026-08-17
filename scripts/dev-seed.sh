#!/usr/bin/env bash
# dev-seed.sh — a throwaway LOCAL multi-org task-server for development.
#
# Unlike dev-clone-prod.sh (which mirrors REAL prod data and needs prod
# credentials you can't type into the web sign-in), this stands up a
# self-contained dev vault with its OWN orgs, its OWN owner account with
# a KNOWN password, and demo content — including the STUDIO DATASET in
# the home org: 50 projects, 3 albums with song sub-projects, the song
# library / setlists / events, and 13 media Files roots with real
# ffmpeg-generated video/audio (ffmpeg must be on PATH for playable
# media; placeholders otherwise). Deterministic names, idempotent +
# healing re-runs. Full guide: docs/dev-seed.md
#
# For the OTHER local world — the example studio as two companies on two
# servers federating over iroh — see scripts/demo.sh (`just demo`).
#
#   ./dev-seed.sh seed     # build + seed the dev vault ($DATA_ROOT)
#   ./dev-seed.sh fresh    # wipe $DATA_ROOT, then seed from scratch
#   ./dev-seed.sh serve    # run task-server against the dev vault
#   ./dev-seed.sh web      # run the web app pointed at the local server
#
# Two terminals for a live app: `serve` in one, `web` in the other, then
# open http://127.0.0.1:8765 and sign in with the printed credentials.
#
# Everything lives under $DATA_ROOT (default
# ~/.local/share/task-dev-seed) — throwaway, never prod. Env overrides:
# TASK_DEV_SEED_ROOT, PORT (server, default 9099), WEB_PORT (default
# 8765), DEV_EMAIL, DEV_PASSWORD.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# `..`, not `../../..`: this script moved to the repo root's `scripts/`
# when the workspace stopped being nested inside `apps/`, and the old
# depth resolved to two directories above the checkout.
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DATA_ROOT="${TASK_DEV_SEED_ROOT:-$HOME/.local/share/task-dev-seed}"
PORT="${PORT:-9099}"
WEB_PORT="${WEB_PORT:-8765}"
DEV_EMAIL="${DEV_EMAIL:-dev@fasttrackstudio.dev}"
DEV_PASSWORD="${DEV_PASSWORD:-password}"

# `cargo run` (not a hardcoded ./target/debug path) so this respects
# CARGO_TARGET_DIR / worktree-local target dirs (PR #295 review).
seed() {
  echo ">> seeding dev vault → $DATA_ROOT"
  ( cd "$REPO_ROOT" && TASK_DATA_ROOT="$DATA_ROOT" \
      cargo run --quiet -p task-server -- admin seed \
      --email "$DEV_EMAIL" --password "$DEV_PASSWORD" )
}

fresh() {
  echo ">> wiping $DATA_ROOT"
  rm -rf "$DATA_ROOT"
  seed
}

serve() {
  if [ ! -d "$DATA_ROOT/orgs" ]; then
    echo "!! $DATA_ROOT not seeded yet — run '$0 seed' first" >&2
    exit 1
  fi
  echo ">> serving $DATA_ROOT on 127.0.0.1:$PORT"
  echo ">>   well-known: http://127.0.0.1:$PORT/.well-known/task-server.json"
  echo ">>   sign in with: $DEV_EMAIL / $DEV_PASSWORD"
  cd "$REPO_ROOT"
  exec env TASK_DATA_ROOT="$DATA_ROOT" TASK_SERVER_BIND="127.0.0.1:$PORT" \
    cargo run --quiet -p task-server
}

web() {
  echo ">> web app on http://127.0.0.1:$WEB_PORT → server ws://127.0.0.1:$PORT/vox"
  cd "$REPO_ROOT/apps/web"
  # Serve WITHOUT hot-patching — dx serve's default hot-patch breaks the
  # wasm build on edits (LinkError/subsecond panic).
  exec env TASK_VOX_URL_WEB="ws://127.0.0.1:$PORT/vox" \
    dx serve --web --addr 127.0.0.1 --port "$WEB_PORT" --hot-patch false
}

[ $# -eq 0 ] && { echo "usage: $0 seed|fresh|serve|web"; exit 1; }
for cmd in "$@"; do
  case "$cmd" in
    seed)  seed ;;
    fresh) fresh ;;
    serve) serve ;;
    web)   web ;;
    *) echo "unknown: $cmd (want seed|fresh|serve|web)" >&2; exit 1 ;;
  esac
done
