#!/usr/bin/env bash
# Migrate one person's email across every org on a Task server.
#
# Each org has its OWN auth store — its own user row, its own id, its own
# session — so a migration is per-org and needs a sign-in per org. This
# asks for the password ONCE and reuses it for each, rather than making
# you type it six times.
#
#   ./migrate-my-email.sh --from cody@codywright.live \
#                         --to acodywright@gmail.com \
#                         [--server wss://task.starcommand.live] \
#                         [--orgs "a b c"] [--reason "..."] [--dry-run]
#
# The password is read with `read -s` into a shell variable and passed to
# `task auth login` for each org. It is never echoed, never written to a
# file, and never placed in argv where `ps` could see it — the CLI reads
# it from TASK_PASSWORD. Your shell history records the flags, not the
# secret.
#
# Idempotent: migrating onto an address the account already holds is a
# no-op server-side, so re-running after a partial failure is safe and
# won't append a history row claiming a change that didn't happen.
set -euo pipefail

SERVER="wss://task.starcommand.live"
ORGS="cbu codywright days-to-praise fasttrackaudio fasttrackstudios tombrooksmusic"
REASON="email migration"
FROM=""
TO=""
DRY=0
TASK_BIN="${TASK_BIN:-task}"

while [ $# -gt 0 ]; do
    case "$1" in
        --from)   FROM="$2"; shift 2 ;;
        --to)     TO="$2"; shift 2 ;;
        --server) SERVER="$2"; shift 2 ;;
        --orgs)   ORGS="$2"; shift 2 ;;
        --reason) REASON="$2"; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

[ -n "$FROM" ] && [ -n "$TO" ] || {
    echo "usage: $0 --from <current-email> --to <new-email> [--server URL] [--orgs \"a b\"] [--reason TEXT] [--dry-run]" >&2
    exit 2
}

command -v "$TASK_BIN" >/dev/null || {
    echo "no \`$TASK_BIN\` on PATH — set TASK_BIN=/path/to/task" >&2
    exit 2
}

echo "Server:  $SERVER"
echo "Migrate: $FROM  ->  $TO"
echo "Orgs:    $ORGS"
[ "$DRY" = 1 ] && echo "(dry run — will sign in and report, but not migrate)"
echo

# Prompted once, held only in this process. Not a script argument, so it
# never appears in `ps` output or shell history.
printf 'Password for %s: ' "$FROM" >&2
read -rs PASSWORD
echo >&2
[ -n "$PASSWORD" ] || { echo "empty password — aborting" >&2; exit 2; }
export TASK_PASSWORD="$PASSWORD"
unset PASSWORD

failed=""
for org in $ORGS; do
    printf '=== %s\n' "$org"

    # No --password flag: the CLI reads TASK_PASSWORD from the
    # environment, so the secret never enters argv where `ps` would show
    # it to every user on the box.
    if ! "$TASK_BIN" --server "$SERVER" --org "$org" auth login \
            --email "$FROM" >/dev/null 2>&1; then
        # Not fatal: an org this account has no user in is a legitimate
        # outcome, not a script failure. Say so and carry on, so one gap
        # doesn't strand the other five.
        echo "  skipped — could not sign in (no account here, or wrong password)"
        failed="$failed $org"
        continue
    fi

    if [ "$DRY" = 1 ]; then
        echo "  signed in OK; would migrate to $TO"
        continue
    fi

    if "$TASK_BIN" --server "$SERVER" --org "$org" auth migrate-email \
            --email "$FROM" --to "$TO" --reason "$REASON"; then
        "$TASK_BIN" --server "$SERVER" --org "$org" auth email-history \
            --email "$TO" 2>/dev/null | sed 's/^/  /'
    else
        echo "  MIGRATION FAILED"
        failed="$failed $org"
    fi
done

unset TASK_PASSWORD
echo
if [ -n "$failed" ]; then
    echo "Incomplete for:$failed"
    echo "Re-running is safe — already-migrated orgs are a no-op."
    exit 1
fi
echo "All orgs migrated to $TO."
echo "Sign in with the NEW address from now on; the old one still resolves"
echo "through the history trail (\`task auth email-history\`)."
