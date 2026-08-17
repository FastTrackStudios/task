#!/usr/bin/env bash
# dev-clone-prod.sh — stand up a LOCAL task-server that mirrors PROD,
# for backend/RPC development you can't do against the deployed server.
#
# Prod stores each org's data as sqlite under a k3s local-path PVC on
# `starcommand` (see the git-backup layout: <data_root>/orgs/<slug>/
# {auth,timer,finance}.sqlite, each an org git repo). This script rsyncs
# that data root to a local clone and runs the server against it.
#
#   ./dev-clone-prod.sh clone         # pull prod data → $DATA_ROOT
#   ./dev-clone-prod.sh serve         # run task-server on 127.0.0.1:$PORT
#   ./dev-clone-prod.sh clone serve   # both
#
# Then, for backend feature work, run the web against the local server:
#   (cd apps/task/web && TASK_VOX_URL_WEB=ws://127.0.0.1:9090/vox \
#      dx serve --web --addr 127.0.0.1 --port 8080)
# Keep the prod-pointed web (wss://task.starcommand.live/vox) only for
# UI-only iterations that don't touch server code.
#
# Env overrides: SSH_HOST (default root@starcommand), DATA_ROOT
# (default ~/task-dev-clone/data), PORT (default 9090), REPO_ROOT
# (default: two levels up from this script).
set -euo pipefail

SSH_HOST="${SSH_HOST:-root@starcommand}"
DATA_ROOT="${DATA_ROOT:-$HOME/task-dev-clone/data}"
PORT="${PORT:-9090}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"

clone() {
  echo ">> discovering prod task-data PVC path on $SSH_HOST…"
  local remote
  remote="$(ssh -o ConnectTimeout=12 "$SSH_HOST" \
    'find /var/lib/rancher/k3s/storage -maxdepth 1 -type d -name "*task-data*" 2>/dev/null | head -1')"
  if [ -z "$remote" ]; then
    echo "!! could not find the task-data PVC on $SSH_HOST" >&2
    exit 1
  fi
  echo ">> prod data: $SSH_HOST:$remote"
  echo ">> rsync → $DATA_ROOT"
  mkdir -p "$DATA_ROOT"
  # Skip the git backup state (.git/.gitstate) and volatile -shm; the
  # -wal is kept so sqlite replays a consistent view on open. NOTE: prod
  # is live, so this is a best-effort hot copy — fine for dev.
  rsync -az --delete \
    --exclude='.git' --exclude='.gitstate' --exclude='*.sqlite-shm' \
    -e "ssh -o ConnectTimeout=12" \
    "$SSH_HOST:$remote/" "$DATA_ROOT/"
  echo ">> done. orgs:"
  ls "$DATA_ROOT/orgs" | sed 's/^/     /'
}

serve() {
  if [ ! -d "$DATA_ROOT/orgs" ]; then
    echo "!! $DATA_ROOT has no orgs/ — run '$0 clone' first" >&2
    exit 1
  fi
  echo ">> building task-server…"
  ( cd "$REPO_ROOT" && cargo build -p task-server )
  echo ">> serving $DATA_ROOT on 127.0.0.1:$PORT (no seed — real prod clone)"
  echo ">>   well-known: http://127.0.0.1:$PORT/.well-known/task-server.json"
  cd "$REPO_ROOT"
  exec env TASK_DATA_ROOT="$DATA_ROOT" TASK_SERVER_BIND="127.0.0.1:$PORT" \
    ./target/debug/task-server
}

[ $# -eq 0 ] && { echo "usage: $0 clone|serve [clone|serve]"; exit 1; }
for cmd in "$@"; do
  case "$cmd" in
    clone) clone ;;
    serve) serve ;;
    *) echo "unknown: $cmd (want clone|serve)"; exit 1 ;;
  esac
done
