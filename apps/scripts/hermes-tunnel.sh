#!/usr/bin/env bash
# Open / close the SSH tunnel to the Hermes dashboard on starcommand.
#
# The dashboard binds to 127.0.0.1:9119 on starcommand and enforces an
# `Invalid Host header` check that rejects any externally-fronted
# hostname. The only working path from a remote laptop is an SSH
# tunnel that makes the dashboard reachable as if it were local.
#
# Usage:
#   ./scripts/hermes-tunnel.sh up      # open the tunnel (background)
#   ./scripts/hermes-tunnel.sh down    # tear it down
#   ./scripts/hermes-tunnel.sh status  # check whether it's up
#
# Forwarded ports:
#   - 9119   hermes-dashboard (the one agent-hermes targets)
#   - 12490  hermes-webui (newer surface; optional but cheap to add)

set -euo pipefail

HOST="${HERMES_TUNNEL_HOST:-root@starcommand}"
DASHBOARD_PORT="${HERMES_DASHBOARD_PORT:-9119}"
WEBUI_PORT="${HERMES_WEBUI_PORT:-12490}"

cmd="${1:-status}"

is_up() {
    pgrep -f "ssh -fN -L ${DASHBOARD_PORT}:127.0.0.1:${DASHBOARD_PORT}" >/dev/null
}

case "$cmd" in
    up)
        if is_up; then
            echo "tunnel already up (pid $(pgrep -f "ssh -fN -L ${DASHBOARD_PORT}"))"
            exit 0
        fi
        ssh -fN \
            -L "${DASHBOARD_PORT}:127.0.0.1:${DASHBOARD_PORT}" \
            -L "${WEBUI_PORT}:127.0.0.1:${WEBUI_PORT}" \
            "$HOST"
        sleep 1
        code=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:${DASHBOARD_PORT}/")
        echo "dashboard http://localhost:${DASHBOARD_PORT}/ -> $code"
        ;;
    down)
        if ! is_up; then
            echo "tunnel not running"
            exit 0
        fi
        pkill -f "ssh -fN -L ${DASHBOARD_PORT}:127.0.0.1:${DASHBOARD_PORT}" || true
        echo "tunnel down"
        ;;
    status)
        if is_up; then
            echo "up — pid $(pgrep -f "ssh -fN -L ${DASHBOARD_PORT}" | head -1)"
            curl -s -o /dev/null -w "  dashboard %{http_code}  (http://localhost:${DASHBOARD_PORT}/)\n" "http://localhost:${DASHBOARD_PORT}/" || echo "  dashboard unreachable"
            curl -s -o /dev/null -w "  webui     %{http_code}  (http://localhost:${WEBUI_PORT}/)\n" "http://localhost:${WEBUI_PORT}/" || echo "  webui unreachable"
        else
            echo "down"
        fi
        ;;
    *)
        echo "usage: $0 {up|down|status}" >&2
        exit 1
        ;;
esac
