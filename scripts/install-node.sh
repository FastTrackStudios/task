#!/bin/sh
# Install (or update) a headless Task node — the `task-server` binary of a
# GitHub Release — on a host without Nix. docs/headless-node.md.
#
#   curl -fsSL https://raw.githubusercontent.com/FastTrackStudios/task/main/scripts/install-node.sh | sh
#   sh install-node.sh [--version X.Y.Z] [--prefix DIR]
#
# Picks the newest v<X.Y.Z> release that carries this platform's binary
# (task-server-linux-x86_64, -linux-aarch64, -macos-aarch64), checks it
# against the release's SHA256SUMS, checks it reports that version, and
# installs it as <prefix>/bin/task-server (default: /usr/local when run as
# root, ~/.local otherwise). Nothing is replaced unless every check passes.
#
# Once installed, `task-server self-update` does the same in place, and
# `task-server service write …` sets up the service and the daily update.
set -eu

REPO="${TASK_REPO:-FastTrackStudios/task}"
version=""
prefix=""

die() { echo "install-node: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs X.Y.Z"; version="${2#v}"; shift 2 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; prefix="$2"; shift 2 ;;
        -h|--help) sed -n '2,15p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option $1" ;;
    esac
done

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64|Linux-amd64) asset=task-server-linux-x86_64 ;;
    Linux-aarch64|Linux-arm64) asset=task-server-linux-aarch64 ;;
    Darwin-arm64|Darwin-aarch64) asset=task-server-macos-aarch64 ;;
    *) die "no release binary for $(uname -s) $(uname -m); build it from source (cargo build --release -p task-server)" ;;
esac

if [ -z "$prefix" ]; then
    if [ "$(id -u)" = 0 ]; then prefix=/usr/local; else prefix="$HOME/.local"; fi
fi
dest="$prefix/bin/task-server"

command -v curl >/dev/null || die "needs curl"
if command -v sha256sum >/dev/null; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "needs sha256sum or shasum"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

# Release tags, newest first (the API lists by date). Only v<X.Y.Z>:
# desktop-v* releases are the desktop updater's.
if [ -n "$version" ]; then
    tags="v$version"
else
    curl -fsSL -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/$REPO/releases?per_page=30" -o "$tmp/releases.json" \
        || die "could not list the releases of $REPO"
    tags="$(grep -o '"tag_name": *"v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*"' "$tmp/releases.json" \
        | sed 's/.*"\(v[^"]*\)"$/\1/')"
    [ -n "$tags" ] || die "no v<X.Y.Z> release found in $REPO"
fi

# The first of them that carries this platform's binary and its checksums.
tag=""
for t in $tags; do
    if curl -fsSL "https://github.com/$REPO/releases/download/$t/SHA256SUMS" -o "$tmp/SHA256SUMS" 2>/dev/null \
        && grep -q " \*\{0,1\}$asset\$" "$tmp/SHA256SUMS"; then
        tag="$t"
        break
    fi
done
[ -n "$tag" ] || die "no release${version:+ v$version} carries $asset yet"
want_version="${tag#v}"

if [ -x "$dest" ] && [ "$("$dest" --version 2>/dev/null || true)" = "task-server $want_version" ]; then
    echo "task-server $want_version is already installed at $dest"
    exit 0
fi

echo "downloading $asset from $tag…"
curl -fSL --progress-bar "https://github.com/$REPO/releases/download/$tag/$asset" -o "$tmp/$asset" \
    || die "download failed"
want="$(grep " \*\{0,1\}$asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
got="$(sha256 "$tmp/$asset")"
[ "$got" = "$want" ] || die "$asset does not match $tag's SHA256SUMS (got $got, want $want); nothing installed"
chmod 755 "$tmp/$asset"
said="$("$tmp/$asset" --version 2>/dev/null || true)"
[ "$said" = "task-server $want_version" ] || die "the downloaded binary says '$said', not 'task-server $want_version'; nothing installed"

mkdir -p "$prefix/bin"
# Stage beside the destination, then rename: atomic, and a running node
# keeps its old binary until it restarts.
cp "$tmp/$asset" "$dest.new.$$"
mv -f "$dest.new.$$" "$dest"
echo "installed $said at $dest"

case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "note: $prefix/bin is not on your PATH" ;;
esac
cat <<EOF

Next (docs/headless-node.md):
  $dest peer sign-in                      sign this node in as your account
  $dest service write <dir>               the service files (systemd or launchd)
EOF
