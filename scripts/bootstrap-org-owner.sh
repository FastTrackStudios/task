#!/usr/bin/env bash
# bootstrap-org-owner.sh — give one account membership + the admin role
# in every org the production server hosts.
#
# WHY THIS EXISTS. An org's auth store is per-org, and
# `AuthService::sign_up_email_password` is member-gated (open signup plus
# the org lane's default `member` role made permission enforcement
# bypassable in one call). So an org with zero accounts has nobody who
# could create the first one, and is unreachable by every client — CLI,
# GUI and agent alike. Five of the six production orgs are in that state:
# nobody can sign into days-to-praise, so its worship songs and setlists
# are invisible to the app.
#
# Possession of the auth store is the only authority that predates any
# account, which is why the bootstrap is a server-binary verb run inside
# the pod rather than an RPC.
#
#   ./bootstrap-org-owner.sh --email you@example.com
#   ./bootstrap-org-owner.sh --email you@example.com --orgs "cbu,tombrooksmusic"
#   ./bootstrap-org-owner.sh --email you@example.com --dry-run
#
# The password is prompted for ONCE, with echo off, and piped straight
# into `kubectl exec -i`. It is never an argument (visible to every user
# on the box via `ps`, and lands in shell history), never written to
# disk, and never echoed.
#
# Idempotent: `create-user` reports and skips an account that already
# exists, so re-running after a partial sweep is safe and finishes the
# rest.
#
# WHAT "OWNER" MEANS HERE. `set-role admin` sets architect-auth's
# `auth_users.role`, which gates its `admin_*` flows. It is NOT the
# permission gate's role — architect-permissions currently hands every
# validated user the same `member` default and never reads that column.
# So admin does not yet widen what the gate allows; membership is what
# actually unlocks the org.
#
# AND NOTE: the same email in several orgs makes several DISTINCT
# accounts with distinct user ids. Auth stores are per-org; cross-org
# identity is phase 3 of the federated-platform work. You share a
# login, not a principal — expect one session entry per org
# (`task auth login --org <slug>` each).
#
# Env overrides: SSH_HOST (default root@starcommand), NAMESPACE, KUBECONFIG.
set -euo pipefail

SSH_HOST="${SSH_HOST:-root@starcommand}"
NAMESPACE="${NAMESPACE:-task}"
KUBECONFIG_PATH="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
EMAIL=""
NAME=""
ORGS=""
DRY_RUN=0

while [ $# -gt 0 ]; do
  case "$1" in
    --email)   EMAIL="${2:-}"; shift 2 ;;
    --name)    NAME="${2:-}"; shift 2 ;;
    --orgs)    ORGS="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) echo "!! unknown flag: $1" >&2; exit 2 ;;
  esac
done

[ -n "$EMAIL" ] || { echo "!! --email is required" >&2; exit 2; }

# `-n` everywhere stdin isn't deliberately piped: ssh reads stdin by
# default, so an unredirected call inside the loop would swallow the
# terminal the password prompt is about to read from.
k() { ssh -n -o ConnectTimeout=12 "$SSH_HOST" "kubectl --kubeconfig=$KUBECONFIG_PATH $*"; }

echo ">> finding the task-server pod in namespace $NAMESPACE on $SSH_HOST…"
POD="$(k get pods -n "$NAMESPACE" -l app.kubernetes.io/name=task-server \
        -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
if [ -z "$POD" ]; then
  # Label schemes drift; fall back to a name match before giving up.
  POD="$(k get pods -n "$NAMESPACE" -o name 2>/dev/null \
          | grep -m1 'task-server' | sed 's|pod/||' || true)"
fi
[ -n "$POD" ] || { echo "!! no task-server pod found in namespace $NAMESPACE" >&2; exit 1; }
echo ">> pod: $POD"

# The server is the authority on which orgs exist — don't hardcode a list
# that will drift the next time one is added.
if [ -z "$ORGS" ]; then
  echo ">> discovering hosted orgs…"
  ORGS="$(curl -fsS https://task.starcommand.live/.well-known/task-server.json \
          | python3 -c 'import json,sys; print(",".join(o["slug"] for o in json.load(sys.stdin)["orgs"]))')"
fi
echo ">> orgs: $ORGS"

if [ "$DRY_RUN" = 1 ]; then
  echo ">> dry run — would create $EMAIL and grant admin in each org above"
  exit 0
fi

# Prompted once, held in a variable, never written anywhere.
printf 'Password for %s (used for every org): ' "$EMAIL" >&2
stty -echo 2>/dev/null || true
IFS= read -r PASSWORD
stty echo 2>/dev/null || true
printf '\n' >&2
[ -n "$PASSWORD" ] || { echo "!! empty password" >&2; exit 2; }

FAILED=""
IFS=',' read -ra SLUGS <<< "$ORGS"
for slug in "${SLUGS[@]}"; do
  slug="$(echo "$slug" | tr -d '[:space:]')"
  [ -n "$slug" ] || continue
  echo
  echo "── $slug ─────────────────────────────────────────────"

  if printf '%s' "$PASSWORD" | ssh -o ConnectTimeout=12 "$SSH_HOST" \
      "kubectl --kubeconfig=$KUBECONFIG_PATH exec -i -n $NAMESPACE $POD -- \
       task-server admin create-user --org $slug --email $EMAIL ${NAME:+--name \"$NAME\"}"
  then :; else
    echo "!! create-user failed for $slug"; FAILED="$FAILED $slug"; continue
  fi

  if ssh -n -o ConnectTimeout=12 "$SSH_HOST" \
      "kubectl --kubeconfig=$KUBECONFIG_PATH exec -n $NAMESPACE $POD -- \
       task-server admin set-role --org $slug --email $EMAIL --role admin"
  then :; else
    echo "!! set-role failed for $slug"; FAILED="$FAILED $slug"
  fi
done

unset PASSWORD
echo
if [ -n "$FAILED" ]; then
  echo "!! finished with failures:$FAILED"
  echo "   the script is idempotent — fix the cause and re-run to finish the rest"
  exit 1
fi
echo ">> done. Sign in per org:"
for slug in "${SLUGS[@]}"; do
  slug="$(echo "$slug" | tr -d '[:space:]')"
  [ -n "$slug" ] && echo "     task auth login --org $slug --email $EMAIL"
done
