#!/usr/bin/env bash
# Build the headless node binary `task-server-linux-x86_64` (node-binaries.yml)
# on a NixOS host: THEBATTLESHIP's `task-release` pool, or by hand.
#
#   scripts/node-binary-linux.sh    # -> dist/task-server-linux-x86_64
#
# The binary must run on hosts without Nix, as ONE file (`task-server
# self-update` and scripts/install-node.sh fetch exactly this asset). So
# OpenSSL (pkgsStatic, from the nixpkgs the flake is locked to) and bzip2
# (bzip2-sys's bundled source) are linked statically, and the interpreter is
# taken off /nix/store: what remains is glibc and libgcc_s, which every distro
# has. Its glibc floor follows the Nix toolchain (decision 2026-10-24), is
# printed, and is refused over MAX_GLIBC.
#
# Env (optional): OUT_DIR (default dist), MAX_GLIBC (default 2.42),
# CARGO_TARGET_DIR (default target).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ -z "${NODE_IN_SHELL:-}" ]; then
  exec nix develop --accept-flake-config "$ROOT#ci" --command env NODE_IN_SHELL=1 bash "$0" "$@"
fi
rev="$(python3 -c "import json;print(json.load(open('flake.lock'))['nodes']['nixpkgs']['locked']['rev'])")"
pk="github:NixOS/nixpkgs/$rev"
if [ -z "${NODE_IN_TOOLS:-}" ]; then
  exec nix shell "$pk#patchelf" "$pk#binutils" --command env NODE_IN_TOOLS=1 bash "$0" "$@"
fi

OUT_DIR="${OUT_DIR:-$ROOT/dist}"
MAX_GLIBC="${MAX_GLIBC:-2.42}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$OUT_DIR"

mapfile -t ssl < <(nix build --no-link --print-out-paths "$pk#pkgsStatic.openssl.out" "$pk#pkgsStatic.openssl.dev")
export OPENSSL_STATIC=1 OPENSSL_LIB_DIR="${ssl[0]}/lib" OPENSSL_INCLUDE_DIR="${ssl[1]}/include"
export BZIP2_NO_PKG_CONFIG=1
echo "=== cargo build --release -p task-server (static OpenSSL ${ssl[0]##*/}, bundled bzip2)"
cargo build --release --locked -p task-server

bin="$OUT_DIR/task-server-linux-x86_64"
cp "$TARGET/release/task-server" "$bin"
chmod 755 "$bin"
patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath "$bin"

needed="$(readelf -d "$bin" | sed -n 's/.*Shared library: \[\(.*\)\].*/\1/p' | tr '\n' ' ')"
echo "=== NEEDED: $needed"
for n in $needed; do
  case "$n" in
    libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | libgcc_s.so.1 | ld-linux-x86-64.so.2) ;;
    *) echo "ERROR: $bin needs $n, which a fresh host may lack" >&2; exit 1 ;;
  esac
done
if readelf -d "$bin" | grep -E 'RPATH|RUNPATH|NEEDED' | grep -q /nix/store ||
  readelf -l "$bin" | grep -q 'interpreter: /nix/store'; then
  echo "ERROR: $bin still points into /nix/store" >&2
  exit 1
fi
floor="$(objdump -p "$bin" | sed -n '/Version References/,$p' | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
echo "=== glibc floor: ${floor:-none} (limit $MAX_GLIBC)"
if [ -n "$floor" ] && [ "$(printf '%s\n%s\n' "$floor" "$MAX_GLIBC" | sort -V | tail -1)" != "$MAX_GLIBC" ]; then
  echo "ERROR: needs glibc $floor, over the limit $MAX_GLIBC" >&2
  exit 1
fi

# It starts and reports the release, through the build's own loader.
ld="$(ldd "$TARGET/release/task-server" | sed -n 's/^\s*\(\/nix\/store\/[^ ]*ld-linux[^ ]*\).*/\1/p' | head -1)"
libdir="$(dirname "$(ldd "$TARGET/release/task-server" | sed -n 's/.*libgcc_s.so.1 => \([^ ]*\).*/\1/p')")"
"$ld" --library-path "$libdir" "$bin" --version
echo "=== $bin ($(du -h "$bin" | cut -f1))"
