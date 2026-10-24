#!/usr/bin/env bash
# Build Task's Linux desktop app (apps/desktop: the window, plus `task` and
# `fts-files-daemon` beside it) as the two Linux packages a release carries:
#
#   dist/Task-linux-x86_64.AppImage    one file, no install; GTK, WebKitGTK
#                                      4.1 and the tray library come from the
#                                      user's system, as they always have
#   dist/Task-linux-x86_64.flatpak     a single-file bundle on the GNOME 51
#                                      runtime (GTK, WebKitGTK 4.1 included);
#                                      `flatpak install Task-linux-x86_64.flatpak`
#
# Runs on THEBATTLESHIP's `task-release` pool (release-assets.yml), or by hand
# on any x86_64 NixOS machine: one release build (`dx build --release`) in the
# repo's dev shell, packaged twice. Studio's scripts/desktop/linux.sh is the
# model.
#
# A binary built in a Nix shell points into /nix/store, which no other
# machine has. So, for each package:
#   - the ELF interpreter becomes /lib64/ld-linux-x86-64.so.2 and RUNPATH
#     $ORIGIN/../lib, and every library the binaries need is either bundled
#     or left to the host (AppImage) / runtime (Flatpak), by family below;
#   - the glibc floor (the newest GLIBC_x.y any shipped ELF requires) is
#     printed, written to dist/, and refused over MAX_GLIBC: a toolchain bump
#     that raises it fails here, not on a user's machine. The floor follows
#     the Nix toolchain (decision 2026-10-24: a recent-distro floor, for one
#     build on our own runner).
#
# Env (optional):
#   VERSION      X.Y.Z (default: the workspace version)
#   OUT_DIR      default dist
#   MAX_GLIBC    default 2.42
#   SKIP_BUILD   1 reuses $CARGO_TARGET_DIR's (or target's) existing build
#   FLATPAK=0    skip the Flatpak
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# The repo's default dev shell (dx, tailwindcss, the GTK/WebKit build env),
# then packaging tools from the nixpkgs the flake is locked to.
if [ -z "${DESKTOP_IN_SHELL:-}" ]; then
  exec nix develop --accept-flake-config "$ROOT#default" \
    --command env DESKTOP_IN_SHELL=1 bash "$0" "$@"
fi
if [ -z "${DESKTOP_IN_TOOLS:-}" ]; then
  rev="$(python3 -c "import json;print(json.load(open('flake.lock'))['nodes']['nixpkgs']['locked']['rev'])" 2>/dev/null || true)"
  pk="nixpkgs"
  [ -n "$rev" ] && pk="github:NixOS/nixpkgs/$rev"
  exec nix shell "$pk#patchelf" "$pk#squashfsTools" "$pk#binutils" "$pk#flatpak" "$pk#ostree" \
    --command env DESKTOP_IN_TOOLS=1 bash "$0" "$@"
fi

VERSION="${VERSION:-$(bash scripts/release.sh version)}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version '$VERSION' is not X.Y.Z" >&2; exit 2; }
OUT_DIR="${OUT_DIR:-$ROOT/dist}"
MAX_GLIBC="${MAX_GLIBC:-2.42}"
APP_ID=app.fasttrackstudio.task
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$OUT_DIR"

# ── Build ───────────────────────────────────────────────────────────────────
if [ "${SKIP_BUILD:-}" != 1 ]; then
  echo "=== dx build --release --linux + task, fts-files-daemon (release) ==="
  bash scripts/architect-ui-link.sh
  (cd apps && tailwindcss -i tailwind.css -o desktop/assets/tailwind.css)
  (cd apps/desktop && dx build --release --linux)
  cargo build --release --locked -p task-app-desktop --bin task --bin fts-files-daemon
fi
app="$(dirname "$(find "$TARGET/dx/task-app-desktop/release" -type f -name task-app-desktop -perm -u+x | head -1)")"
[ -d "$app/assets" ] || { echo "ERROR: dx wrote no assets/ beside the binary at '$app'" >&2; exit 1; }
BINS=("$app/task-app-desktop" "$TARGET/release/task" "$TARGET/release/fts-files-daemon")
for b in "${BINS[@]}"; do [ -x "$b" ] || { echo "ERROR: no $b" >&2; exit 1; }; done

WORK="$(mktemp -d -p "$OUT_DIR" .linux.XXXXXX)" # on the build disk, not /tmp
trap 'rm -rf "$WORK"' EXIT

# Every library the three binaries can load, name -> /nix/store path.
declare -A where=()
for b in "${BINS[@]}"; do
  while read -r name arrow path _; do
    [ "$arrow" = "=>" ] && [ -f "${path:-}" ] && where[$name]="$path"
  done < <(ldd "$b")
done

# Libraries a package never carries: glibc (the user's), the graphics stack
# (driver-matched), and the toolkit stack GTK/WebKitGTK hold together (one
# copy, the host's or the runtime's: WebKitGTK spawns helper processes from
# its own install, so it can't be relocated into a bundle).
glibc_or_gpu() {
  case "$1" in
    libc.so* | libm.so* | libdl.so* | libpthread.so* | librt.so* | libresolv.so* | libutil.so* | libmvec.so* | libanl.so* | libnsl.so* | libnss_* | ld-linux*) return 0 ;;
    libGL.so* | libGLX* | libGLdispatch* | libOpenGL* | libEGL* | libGLESv* | libvulkan* | libdrm* | libgbm*) return 0 ;;
    *) return 1 ;;
  esac
}
toolkit() {
  case "$1" in
    libwebkit2gtk-* | libjavascriptcoregtk-* | libgtk-3* | libgdk-3* | libgdk_pixbuf-* | libglib-2* | libgobject-2* | libgio-2* | libgmodule-2* | libgthread-2*) return 0 ;;
    libsoup-3* | libpango* | libcairo* | libharfbuzz* | libatk* | libatspi* | libfontconfig* | libfreetype* | libepoxy*) return 0 ;;
    libX* | libxcb* | libxkbcommon* | libwayland-* | libstdc++* | libgcc_s*) return 0 ;;
    libsystemd* | libdbus-1* | libz.so*) return 0 ;;
    *) return 1 ;;
  esac
}
tray() { case "$1" in libayatana-* | libdbusmenu-* | libappindicator*) return 0 ;; *) return 1 ;; esac; }

# The tray library is dlopen()ed (libappindicator-sys), never NEEDED, so no
# ldd walk finds it: the AppImage leaves it to the host, as it always has;
# the GNOME runtime has none, so the Flatpak carries it and its own chain.
TRAY="$(pkg-config --variable=libdir ayatana-appindicator3-0.1)/libayatana-appindicator3.so.1"
[ -f "$TRAY" ] || { echo "ERROR: no libayatana-appindicator3 in the dev shell ($TRAY)" >&2; exit 1; }
while read -r name arrow path _; do
  [ "$arrow" = "=>" ] && [ -f "${path:-}" ] && where[$name]="$path"
done < <(ldd "$TRAY")
where[libayatana-appindicator3.so.1]="$TRAY"

# The libraries to bundle: start from what the binaries NEED directly and
# follow NEEDED edges, never into a library the host/runtime provides.
# $1: "appimage" (tray from the host) or "flatpak" (tray bundled).
closure() {
  local kind="$1" f n
  local -A take=()
  local -a queue=("${BINS[@]}")
  if [ "$kind" = flatpak ]; then
    take[libayatana-appindicator3.so.1]="$TRAY"
    queue+=("$TRAY")
  fi
  while [ "${#queue[@]}" -gt 0 ]; do
    f="${queue[0]}"
    queue=("${queue[@]:1}")
    while read -r n; do
      n="$(basename "$n")"
      glibc_or_gpu "$n" && continue
      toolkit "$n" && continue
      [ "$kind" = appimage ] && tray "$n" && continue
      [ -n "${take[$n]:-}" ] && continue
      [ -n "${where[$n]:-}" ] || { echo "ERROR: $(basename "$f") needs $n, which ldd did not resolve" >&2; exit 1; }
      take[$n]="${where[$n]}"
      queue+=("${where[$n]}")
    done < <(readelf -d "$f" | sed -n 's/.*Shared library: \[\(.*\)\].*/\1/p')
  done
  for n in "${!take[@]}"; do printf '%s\t%s\n' "$n" "${take[$n]}"; done | sort
}

# Lay out bin/ + lib/ under $1 (the binaries and their assets/ in bin/), with
# every ELF taken off /nix/store.
populate() { # <prefix> <appimage|flatpak>
  local pre="$1" kind="$2" n p f
  mkdir -p "$pre/bin" "$pre/lib"
  cp -R "$app/." "$pre/bin/"
  install -m755 "$TARGET/release/task" "$TARGET/release/fts-files-daemon" "$pre/bin/"
  closure "$kind" > "$WORK/$kind.libs"
  while IFS=$'\t' read -r n p; do
    cp -L "$p" "$pre/lib/$n"
    chmod u+w "$pre/lib/$n"
  done < "$WORK/$kind.libs"
  echo "=== $kind: bundled $(wc -l < "$WORK/$kind.libs") libraries: $(cut -f1 "$WORK/$kind.libs" | tr '\n' ' ')"
  for f in "$pre"/bin/task-app-desktop "$pre"/bin/task "$pre"/bin/fts-files-daemon; do
    chmod u+w "$f"
    # shellcheck disable=SC2016 # $ORIGIN is for the dynamic loader
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --set-rpath '$ORIGIN/../lib' "$f"
  done
  for f in "$pre"/lib/*; do
    [ -e "$f" ] || continue
    # shellcheck disable=SC2016
    patchelf --set-rpath '$ORIGIN' "$f"
  done
  # A NEEDED entry spelled as a /nix/store path becomes its base name.
  for f in "$pre"/bin/task-app-desktop "$pre"/bin/task "$pre"/bin/fts-files-daemon "$pre"/lib/*; do
    [ -e "$f" ] || continue
    readelf -d "$f" | sed -n 's/.*Shared library: \[\(\/[^]]*\)\].*/\1/p' | while read -r abs; do
      patchelf --replace-needed "$abs" "$(basename "$abs")" "$f"
    done
  done
  for f in "$pre"/bin/task-app-desktop "$pre"/bin/task "$pre"/bin/fts-files-daemon "$pre"/lib/*; do
    [ -e "$f" ] || continue
    if readelf -d "$f" | grep -E 'RPATH|RUNPATH|NEEDED' | grep -q /nix/store ||
      readelf -l "$f" 2>/dev/null | grep -q 'interpreter: /nix/store'; then
      echo "ERROR: $f still points into /nix/store" >&2
      exit 1
    fi
  done
}

# The newest GLIBC_x.y any shipped ELF REQUIRES; refused over MAX_GLIBC.
glibc_floor() { # <prefix> <label>
  local pre="$1" label="$2" floor
  floor="$(for f in "$pre"/bin/task-app-desktop "$pre"/bin/task "$pre"/bin/fts-files-daemon "$pre"/lib/*; do
    [ -e "$f" ] && objdump -p "$f" 2>/dev/null | sed -n '/Version References/,$p' | grep -o 'GLIBC_[0-9][0-9.]*'
  done | sed 's/GLIBC_//' | sort -uV | tail -1)"
  echo "=== $label glibc floor: ${floor:-none} (limit $MAX_GLIBC)"
  if [ -n "$floor" ] && [ "$(printf '%s\n%s\n' "$floor" "$MAX_GLIBC" | sort -V | tail -1)" != "$MAX_GLIBC" ]; then
    echo "ERROR: $label needs glibc $floor, over the limit $MAX_GLIBC" >&2
    exit 1
  fi
  echo "$floor" > "$OUT_DIR/$label-glibc-floor.txt"
}

# The patched window starts and reports the release, through the build's
# own loader and libraries (on any host; `--version` opens no window).
smoke() { # <bin dir> <lib dir> <label>
  local bin="$1" lib="$2" label="$3" ld libpath got
  ld="$(ldd "${BINS[0]}" | sed -n 's/^\s*\(\/nix\/store\/[^ ]*ld-linux[^ ]*\).*/\1/p' | head -1)"
  libpath="$lib:$(for p in "${where[@]}"; do dirname "$p"; done | sort -u | tr '\n' ':')"
  got="$(env -u DISPLAY -u WAYLAND_DISPLAY "$ld" --library-path "$libpath" "$bin/task-app-desktop" --version)"
  [ "$got" = "task-app-desktop $VERSION" ] || { echo "ERROR: $label says '$got', not 'task-app-desktop $VERSION'" >&2; exit 1; }
  echo "=== $label: $got"
}

icon=apps/web/assets/icon-512.png
desktop_entry() { # <Exec> <Icon>
  printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=Task' 'Comment=FastTrackStudio Task' \
    "Exec=$1" "Icon=$2" 'Categories=Office;ProjectManagement;' 'Terminal=false' \
    'StartupWMClass=task-app-desktop'
}

# ── AppImage ────────────────────────────────────────────────────────────────
AD="$WORK/Task.AppDir"
populate "$AD/usr" appimage
glibc_floor "$AD/usr" Task-linux-x86_64.AppImage
smoke "$AD/usr/bin" "$AD/usr/lib" AppImage
mkdir -p "$AD/usr/share/applications" "$AD/usr/share/icons/hicolor/512x512/apps"
{ desktop_entry task-app-desktop task; echo "X-AppImage-Version=$VERSION"; } > "$AD/task.desktop"
cp "$AD/task.desktop" "$AD/usr/share/applications/task.desktop"
cp "$icon" "$AD/task.png"
cp "$icon" "$AD/usr/share/icons/hicolor/512x512/apps/task.png"
cp "$icon" "$AD/.DirIcon"
# Compositing off by default, as the nix package does (a white window on
# some GPUs); a user's own setting wins.
cat > "$AD/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
: "${WEBKIT_DISABLE_COMPOSITING_MODE:=1}"
export WEBKIT_DISABLE_COMPOSITING_MODE
exec "$HERE/usr/bin/task-app-desktop" "$@"
APPRUN
chmod 755 "$AD/AppRun"

# The type2 runtime with the AppDir's squashfs appended: what appimagetool
# does. The runtime is a pinned upstream release, checked against its sha256.
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64"
RUNTIME_SHA256="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"
RT="${XDG_CACHE_HOME:-$HOME/.cache}/fts-appimage/runtime-x86_64-20251108"
mkdir -p "$(dirname "$RT")"
if ! { [ -f "$RT" ] && echo "$RUNTIME_SHA256  $RT" | sha256sum -c --status; }; then
  curl -fsSL --retry 3 -o "$RT.part" "$RUNTIME_URL"
  echo "$RUNTIME_SHA256  $RT.part" | sha256sum -c --status ||
    { echo "ERROR: the AppImage runtime does not match its pinned sha256" >&2; rm -f "$RT.part"; exit 1; }
  mv "$RT.part" "$RT"
fi
mksquashfs "$AD" "$WORK/app.squashfs" -root-owned -noappend -comp zstd -Xcompression-level 15 -no-xattrs -quiet
OUT="$OUT_DIR/Task-linux-x86_64.AppImage"
cat "$RT" "$WORK/app.squashfs" > "$OUT"
chmod 755 "$OUT"
echo "=== $OUT ($(du -h "$OUT" | cut -f1))"

# ── Flatpak ─────────────────────────────────────────────────────────────────
# Assembled with flatpak's build-init/-finish/-export/-bundle, none of which
# runs anything in a sandbox (so it works in the runner unit, which forbids
# user namespaces). The runtime is only needed as a NAME here; a user's
# `flatpak install` fetches it from Flathub.
if [ "${FLATPAK:-1}" != 0 ]; then
  RUNTIME=org.gnome.Platform
  SDK=org.gnome.Sdk
  BRANCH=51
  FB="$WORK/flatpak-build"
  REPO="$WORK/flatpak-repo"
  # What `flatpak build-init` writes, by hand: build-init insists the SDK is
  # installed, and nothing after it needs either.
  mkdir -p "$FB/files" "$FB/var"
  printf '[Application]\nname=%s\nruntime=%s/x86_64/%s\nsdk=%s/x86_64/%s\n' \
    "$APP_ID" "$RUNTIME" "$BRANCH" "$SDK" "$BRANCH" > "$FB/metadata"
  populate "$FB/files" flatpak
  glibc_floor "$FB/files" Task-linux-x86_64.flatpak
  smoke "$FB/files/bin" "$FB/files/lib" Flatpak
  mkdir -p "$FB/files/share/applications" "$FB/files/share/icons/hicolor/512x512/apps"
  desktop_entry task-app-desktop "$APP_ID" > "$FB/files/share/applications/$APP_ID.desktop"
  cp "$icon" "$FB/files/share/icons/hicolor/512x512/apps/$APP_ID.png"
  flatpak build-finish "$FB" --command=task-app-desktop \
    --share=network --share=ipc --socket=wayland --socket=fallback-x11 --device=dri \
    --filesystem=home --talk-name=org.kde.StatusNotifierWatcher \
    --talk-name=org.freedesktop.Notifications --talk-name=org.freedesktop.secrets \
    --env=WEBKIT_DISABLE_COMPOSITING_MODE=1 >/dev/null
  flatpak build-export "$REPO" "$FB" stable >/dev/null
  FP="$OUT_DIR/Task-linux-x86_64.flatpak"
  flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$REPO" "$FP" "$APP_ID" stable
  echo "=== $FP ($(du -h "$FP" | cut -f1))"
fi

(cd "$OUT_DIR" && sha256sum Task-linux-x86_64.* | grep -v glibc > SHA256SUMS-linux-desktop)
echo "=== done:"
cat "$OUT_DIR/SHA256SUMS-linux-desktop"
