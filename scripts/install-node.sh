#!/bin/sh
# Install (or update) a headless Task node — the `task-server` binary of a
# GitHub Release — on a host without Nix. docs/headless-node.md.
#
#   curl -fsSL https://raw.githubusercontent.com/FastTrackStudios/task/main/scripts/install-node.sh | sh
#   sh install-node.sh [--version X.Y.Z] [--prefix DIR] [--unsigned]
#
# Picks the newest v<X.Y.Z> release that carries this platform's binary
# (task-server-linux-x86_64, -linux-aarch64, -macos-aarch64), checks the
# release's SHA256SUMS.minisig against the release key below when
# `minisign` is on PATH, checks the binary against SHA256SUMS, checks it
# reports that version, and installs it as <prefix>/bin/task-server
# (default: /usr/local when run as root, ~/.local otherwise). Nothing is
# replaced unless every check passes. Without minisign the checksums are
# only integrity: the first install trusts GitHub (trust on first use).
# --unsigned installs a release from before signing (v0.2.4 and older).
#
# Once installed, `task-server self-update` does the same in place, and
# `task-server service write …` sets up the service and the daily update.
set -eu

# The release signing key: the minisign public key whose secret half signs
# every release's SHA256SUMS. The same as RELEASE_SIGNING_KEY in
# apps/server/src/self_update.rs (a unit test keeps them equal).
RELEASE_KEY="RWRh9Gk9qaStQMhku4THMidEYrQsmAEsm2dDprUkZxShEtqfIcroM8Od"

REPO="${TASK_REPO:-FastTrackStudios/task}"
version=""
prefix=""
unsigned=""

die() { echo "install-node: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs X.Y.Z"; version="${2#v}"; shift 2 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; prefix="$2"; shift 2 ;;
        --unsigned) unsigned=1; shift ;;
        -h|--help) sed -n '2,19p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
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

# The signature is what makes SHA256SUMS mean anything: whoever can write
# a release's assets can replace the binary and its checksum together.
if command -v minisign >/dev/null; then
    if curl -fsSL "https://github.com/$REPO/releases/download/$tag/SHA256SUMS.minisig" \
        -o "$tmp/SHA256SUMS.minisig" 2>/dev/null; then
        verified="$(minisign -V -P "$RELEASE_KEY" -m "$tmp/SHA256SUMS" -x "$tmp/SHA256SUMS.minisig" 2>&1)" \
            || die "$tag's SHA256SUMS is not signed by the release key ($verified); nothing installed"
        # The trusted comment names the release, so a signature lifted from
        # an older release (a rollback) is refused.
        printf '%s\n' "$verified" | grep -qxF "Trusted comment: task-server $tag SHA256SUMS" \
            || die "$tag's SHA256SUMS.minisig was made for another release ($verified); nothing installed"
        echo "$tag's SHA256SUMS is signed by the release key"
    elif [ -n "$unsigned" ]; then
        echo "warning: $tag carries no SHA256SUMS.minisig; installing on the checksums alone (--unsigned)" >&2
    else
        die "$tag carries no SHA256SUMS.minisig (releases before v0.2.5 are unsigned); pass --unsigned to install it on the checksums alone"
    fi
else
    echo "note: minisign is not on PATH, so $tag's signature is not checked: SHA256SUMS proves" >&2
    echo "      the download is intact, not who built it. This first install trusts GitHub" >&2
    echo "      (trust on first use); task-server self-update checks the signature from then on." >&2
    echo "      Install minisign (https://jedisct1.github.io/minisign/) and re-run to check it now." >&2
fi

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
