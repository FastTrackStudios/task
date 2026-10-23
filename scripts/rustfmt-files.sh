#!/usr/bin/env bash
# Format exactly the given Rust files (prek's rustfmt hook passes the
# staged ones), each through rustfmt's stdin. `rustfmt <file>` would
# follow `mod` declarations into files that aren't staged; stdin never
# does. The edition is the workspace's.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

edition="$(sed -n 's/^edition *= *"\([0-9]*\)".*/\1/p' Cargo.toml | head -1)"
rc=0
for f in "$@"; do
  tmp="$(mktemp)"
  if rustfmt --edition "${edition:-2024}" --emit stdout <"$f" >"$tmp"; then
    cmp -s "$f" "$tmp" || cat "$tmp" >"$f"
  else
    echo "rustfmt: could not format $f" >&2
    rc=1
  fi
  rm -f "$tmp"
done
exit "$rc"
