#!/usr/bin/env bash
set -euo pipefail

# Point git at this directory for hooks. Run once per clone:
#
#   ./.githooks/install.sh          (or `just install-hooks`)
#
# `core.hooksPath` is repo-global and applies to every worktree, so
# there is nothing to copy and nothing to re-run after `git worktree
# add`. It is also the ONLY thing this script does — the hooks
# themselves are the tracked files in this directory.
#
# Installing these makes `capn fmt` / `capn pre-push` run for every
# commit in this repo. Uninstall with:
#
#   git config --unset core.hooksPath

HOOK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git rev-parse --show-toplevel)"
REL="${HOOK_DIR#"$REPO_ROOT"/}"

chmod +x "$HOOK_DIR"/pre-commit "$HOOK_DIR"/pre-push
git config core.hooksPath "$REL"
echo "Configured git core.hooksPath=$REL"
