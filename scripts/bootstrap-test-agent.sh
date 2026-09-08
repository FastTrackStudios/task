#!/usr/bin/env bash
# bootstrap-test-agent.sh — one shared FastTrackStudio identity for
# agents, and everything the Task server needs to believe it.
#
#   ./scripts/bootstrap-test-agent.sh --dry-run
#   ./scripts/bootstrap-test-agent.sh
#
# WHY THIS EXISTS. An agent that verifies anything against production
# needs an identity. Doing it by hand is four steps across three systems,
# and getting any of them wrong leaves either a credential with more
# reach than testing needs, or an account that authenticates perfectly
# and can read nothing. It also, done by hand, means typing a password —
# which is the one thing that must not happen over a chat transcript.
#
# So this script types nothing and prompts for nothing. It GENERATES the
# password, uses it, stores it 0600, and never prints it. Nothing secret
# reaches your terminal, your shell history, or any conversation you run
# this from. Run it and read the summary; there is nothing to copy back.
#
# WHAT IT BUILDS, and why it takes four steps rather than one:
#
#   1. An account at the ISSUER (auth.fasttrackstudio.app). This is the
#      identity itself — one account, shared by every agent, resolvable
#      by any FastTrackStudio app.
#
#   2. A LOCAL account with the same address in each named org. This
#      looks redundant and is not. `adopt-principal` grants membership
#      by finding orgs that already hold an account with that address —
#      it copies each org's own role onto the principal. With no local
#      account there is nothing to adopt, and the principal gets no rows.
#
#   3. `adopt-principal --principal <issuer uuid>`, keying those rows to
#      the ISSUER's id rather than any local one. This is the step that
#      makes an issuer-minted token mean something here: `central_auth`
#      resolves a token to the issuer's uuid, and a membership row keyed
#      to a local uuid would never be found. Getting this wrong is the
#      failure that looks like "the token is valid and the account
#      belongs nowhere".
#
#   4. A session file of its own, so an agent using TASK_SESSION_FILE can
#      never act as you by accident.
#
# SCOPE. Default is `codywright` alone. Widen it only as far as the
# testing needs — this account is shared by every agent, so its reach is
# every agent's reach. Re-running is safe (create-user reports and skips
# an existing account), so adding an org later is a re-run, not a rebuild.
#
# Env overrides: ISSUER, SERVER, SSH_HOST, NAMESPACE, KUBECONFIG,
# EMAIL, NAME, ORGS, AGENT_DIR.
set -euo pipefail

ISSUER="${ISSUER:-https://auth.fasttrackstudio.app}"
SERVER="${SERVER:-https://task.fasttrackstudio.app}"
SSH_HOST="${SSH_HOST:-root@starcommand}"
NAMESPACE="${NAMESPACE:-task}"
KUBECONFIG_PATH="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
EMAIL="${EMAIL:-agent@fasttrackstudio.app}"
NAME="${NAME:-Test Agent}"
ORGS="${ORGS:-codywright}"
AGENT_DIR="${AGENT_DIR:-$HOME/.local/share/task-agent}"
DRY_RUN=0

while [ $# -gt 0 ]; do
  case "$1" in
    --email)   EMAIL="${2:-}"; shift 2 ;;
    --name)    NAME="${2:-}"; shift 2 ;;
    --orgs)    ORGS="${2:-}"; shift 2 ;;
    --issuer)  ISSUER="${2:-}"; shift 2 ;;
    --server)  SERVER="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) sed -n '2,48p' "$0"; exit 0 ;;
    *) echo "!! unknown flag: $1" >&2; exit 2 ;;
  esac
done

for tool in curl jq openssl ssh; do
  command -v "$tool" >/dev/null || { echo "!! missing required tool: $tool" >&2; exit 2; }
done

CREDS="$AGENT_DIR/credentials"
SESSION="$AGENT_DIR/session.json"
WS_SERVER="$(printf '%s' "$SERVER" | sed 's|^http|ws|')"

echo "── plan ──────────────────────────────────────────────"
echo "   identity : $EMAIL  (\"$NAME\")"
echo "   issuer   : $ISSUER"
echo "   server   : $SERVER"
echo "   orgs     : $ORGS"
echo "   secrets  : $CREDS  (0600, generated, never printed)"
echo "   session  : $SESSION"
echo

if [ "$DRY_RUN" = 1 ]; then
  echo ">> dry run — nothing was created."
  echo "   Re-run without --dry-run to build it."
  exit 0
fi

# `-n` everywhere stdin is not deliberately piped: ssh reads stdin by
# default and would otherwise swallow the password being fed to a later
# command in the same pipeline.
k() { ssh -n -o ConnectTimeout=12 "$SSH_HOST" "kubectl --kubeconfig=$KUBECONFIG_PATH $*"; }

echo ">> locating the task-server pod in namespace $NAMESPACE…"
POD="$(k get pods -n "$NAMESPACE" -l app.kubernetes.io/name=task-server \
        -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
if [ -z "$POD" ]; then
  POD="$(k get pods -n "$NAMESPACE" -o name 2>/dev/null \
          | grep -m1 'task-server' | sed 's|pod/||' || true)"
fi
[ -n "$POD" ] || { echo "!! no task-server pod found in namespace $NAMESPACE" >&2; exit 1; }
echo "   pod: $POD"

# ── The credential ───────────────────────────────────────────────────
# Generated here, never typed and never displayed. 32 bytes of urandom
# through base64 is ~43 characters of entropy no human has to remember,
# which is the point: nobody types this, so nothing is gained by making
# it memorable and a great deal is lost.
#
# Written before it is used, and with the directory locked down first —
# a credential that exists on the wire but not on disk is one nobody can
# ever use again.
mkdir -p "$AGENT_DIR"
chmod 700 "$AGENT_DIR"
umask 077
PASSWORD="$(openssl rand -base64 32 | tr -d '\n=' )"

# ── 1. The issuer account ────────────────────────────────────────────
echo ">> creating the issuer account at $ISSUER…"
SIGNUP="$(curl -fsS -X POST "$ISSUER/auth/sign-up/email" \
  -H 'content-type: application/json' \
  --data-binary @<(jq -n --arg e "$EMAIL" --arg p "$PASSWORD" --arg n "$NAME" \
                      '{email:$e, password:$p, name:$n}') 2>/dev/null || true)"

PRINCIPAL="$(printf '%s' "$SIGNUP" | jq -r '.user.id // empty' 2>/dev/null || true)"
TOKEN="$(printf '%s' "$SIGNUP" | jq -r '.token // empty' 2>/dev/null || true)"

if [ -z "$PRINCIPAL" ]; then
  # Sign-up refuses an address it already holds, which is the ordinary
  # state on a re-run. Sign in instead — but only the ORIGINAL password
  # works, so a re-run after a lost credentials file cannot recover and
  # says so rather than leaving a half-built identity.
  echo "   sign-up did not return a user; trying sign-in (the account may already exist)…"
  if [ -r "$CREDS" ]; then
    PASSWORD="$(jq -r '.password' "$CREDS")"
  fi
  SIGNIN="$(curl -fsS -X POST "$ISSUER/auth/sign-in/email" \
    -H 'content-type: application/json' \
    --data-binary @<(jq -n --arg e "$EMAIL" --arg p "$PASSWORD" \
                        '{email:$e, password:$p}') 2>/dev/null || true)"
  PRINCIPAL="$(printf '%s' "$SIGNIN" | jq -r '.user.id // empty' 2>/dev/null || true)"
  TOKEN="$(printf '%s' "$SIGNIN" | jq -r '.token // empty' 2>/dev/null || true)"
fi

if [ -z "$PRINCIPAL" ] || [ -z "$TOKEN" ]; then
  echo "!! the issuer neither created nor recognised $EMAIL." >&2
  echo "   If this account exists with a password this script did not generate," >&2
  echo "   reset it at $ISSUER, or pick another address with --email." >&2
  exit 1
fi
echo "   principal: $PRINCIPAL"

# Persist before anything else can fail. The password is the only thing
# that cannot be re-derived.
jq -n --arg e "$EMAIL" --arg p "$PASSWORD" --arg i "$ISSUER" --arg u "$PRINCIPAL" \
   '{email:$e, password:$p, issuer:$i, principal:$u}' > "$CREDS"
chmod 600 "$CREDS"
echo "   credential written to $CREDS (0600)"

# ── 2. A local account per org, so there is something to adopt ───────
FAILED=""
IFS=',' read -ra SLUGS <<< "$ORGS"
for slug in "${SLUGS[@]}"; do
  slug="$(printf '%s' "$slug" | tr -d '[:space:]')"
  [ -n "$slug" ] || continue
  echo ">> local account in $slug…"
  if printf '%s' "$PASSWORD" | ssh -o ConnectTimeout=12 "$SSH_HOST" \
      "kubectl --kubeconfig=$KUBECONFIG_PATH exec -i -n $NAMESPACE $POD -- \
       task-server admin create-user --org $slug --email $EMAIL --name \"$NAME\""
  then :; else
    echo "!! create-user failed for $slug"; FAILED="$FAILED $slug"
  fi
done

# ── 3. Key the membership rows to the ISSUER's id ────────────────────
# The step that makes an issuer token mean something here. Without
# --principal the rows are keyed to a local uuid, and a token resolving
# to the issuer's uuid would match none of them.
echo ">> adopting $PRINCIPAL as the principal for $EMAIL…"
k exec -n "$NAMESPACE" "$POD" -- \
  task-server admin adopt-principal --email "$EMAIL" --principal "$PRINCIPAL"

# ── 4. A session of its own ──────────────────────────────────────────
# TWO files, and getting this wrong fails silently in the worst way.
#
# The routing document holds only `{url, slug}` per key. The TOKEN lives
# in a sibling `<stem>-tokens/<key>.json`, because `session_store::load`
# reads the routing doc and then fetches each key's token from that
# directory — and **a key whose token file is missing is DROPPED**, on
# the reasoning that it was signed out out-of-band.
#
# So a session file with the token written inline looks completely
# correct, parses, prints fine under `task auth whoami`, and yet every
# command goes out with no `Authorization` header at all. The server then
# says "anonymous is not a member", which reads like a permissions or
# adoption problem and is neither. That cost a full debugging session:
# the server's own span said `auth.token_presented: false`, which is what
# finally pointed here rather than at the membership rows.
first_org="$(printf '%s' "$ORGS" | cut -d, -f1 | tr -d '[:space:]')"
key="$first_org@$(printf '%s' "$SERVER" | sed -e 's|^https\?://||')"
tokens_dir="$AGENT_DIR/$(basename "$SESSION" .json)-tokens"

jq -n --arg k "$key" --arg url "$WS_SERVER" --arg slug "$first_org" \
   '{home:$k, active:$k, servers: {($k): {url:$url, slug:$slug}}}' > "$SESSION"
chmod 600 "$SESSION"

mkdir -p "$tokens_dir"
chmod 700 "$tokens_dir"
# `expires_at_unix` is advisory here: this is a fresh sign-in, and the
# issuer is the authority on whether the token still works. A week is a
# plausible horizon that does not pretend to more precision than the
# sign-up response gave us.
jq -n --arg t "$TOKEN" --arg e "$EMAIL" --arg u "$PRINCIPAL" \
      --argjson x "$(( $(date +%s) + 604800 ))" \
   '{token:$t, email:$e, user_id:$u, expires_at_unix:$x}' > "$tokens_dir/$key.json"
chmod 600 "$tokens_dir/$key.json"

echo
echo "── done ──────────────────────────────────────────────"
echo "   principal : $PRINCIPAL"
[ -n "$FAILED" ] && echo "   !! no local account in:$FAILED (adopt granted them nothing)"
echo
echo "   Drive the agent with:"
echo "     export TASK_SESSION_FILE=$SESSION"
echo "     task chart list"
echo
echo "   Your own session store is untouched — check with: task auth whoami"
echo "   The password was generated and stored; it was never displayed."
