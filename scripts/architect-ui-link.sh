#!/usr/bin/env bash
# Point apps/.architect-ui at the architect-ui source cargo resolved for
# Cargo.lock's pin, or (--check) prove Task's sheet comes from there.
#
# Task's design tokens are architect-ui's `assets/fts-theme.css`, and the
# components drawn from architect-ui (toasts, sheets, badges) carry their
# own utility classes. Tailwind resolves `@import` and `@source` on the
# filesystem, and architect-ui is a git dependency with no stable path, so
# `apps/fts-theme.css` imports, and `apps/tailwind.css` scans, the
# gitignored symlink `apps/.architect-ui`. This script writes it from
# `cargo metadata`, the same trick studio's Justfile uses. A missing link
# fails the Tailwind build loudly ("Can't resolve"); it never falls back to
# a stale copy, because there is no copy any more.
#
# Run it before every `tailwindcss` call (`just css` does). Inside a nix
# build cargo is offline with vendored sources; the `--offline` retry
# covers that.
#
#   scripts/architect-ui-link.sh          write/refresh the link
#   scripts/architect-ui-link.sh --check  fail on drift (CI: `just ci-architect-css`)
#
# --check fails when:
#   - the link is missing or points anywhere but the locked architect-ui
#     (a checkout left over from an older pin is drift too);
#   - apps/fts-theme.css no longer @imports the linked sheet;
#   - any tracked .css file is a byte copy of the canonical sheet (a
#     re-vendored token sheet, the thing STA-392 deleted).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
link=apps/.architect-ui
import_line='@import "./.architect-ui/assets/fts-theme.css";'

resolve() {
    local meta
    meta=$(cargo metadata --locked --format-version 1 2>/dev/null) \
        || meta=$(cargo metadata --locked --offline --format-version 1)
    printf '%s' "$meta" | python3 -c '
import json, os, sys
pkgs = [p for p in json.load(sys.stdin)["packages"] if p["name"] == "architect-ui"]
if len(pkgs) != 1:
    sys.exit(f"architect-ui: expected one package in the lock, found {len(pkgs)}")
print(os.path.dirname(pkgs[0]["manifest_path"]))
'
}

dir=$(resolve)
if [ ! -f "$dir/assets/fts-theme.css" ]; then
    echo "architect-ui at $dir has no assets/fts-theme.css" >&2
    exit 1
fi

if [ "${1:-}" != "--check" ]; then
    ln -sfn "$dir" "$link"
    echo "$link -> $dir"
    exit 0
fi

fail=0
if [ ! -L "$link" ]; then
    echo "drift: $link is missing; run scripts/architect-ui-link.sh (just css)" >&2
    fail=1
elif [ "$(readlink -f "$link")" != "$(readlink -f "$dir")" ]; then
    echo "drift: $link -> $(readlink "$link"), but Cargo.lock resolves architect-ui to $dir; run just css" >&2
    fail=1
fi

if ! grep -qxF "$import_line" apps/fts-theme.css; then
    echo "drift: apps/fts-theme.css must contain the line: $import_line" >&2
    fail=1
fi

canon=$(sha256sum <"$dir/assets/fts-theme.css" | cut -d' ' -f1)
while IFS= read -r f; do
    [ -f "$f" ] || continue
    if [ "$(sha256sum <"$f" | cut -d' ' -f1)" = "$canon" ]; then
        echo "drift: $f is a vendored copy of architect-ui's fts-theme.css; @import $link instead" >&2
        fail=1
    fi
done < <(git ls-files -- '*.css')

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "architect-ui sheet: one source ($dir), imported by apps/fts-theme.css"
