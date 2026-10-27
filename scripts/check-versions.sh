#!/usr/bin/env bash
# One version everywhere: every artifact a release ships reports the
# release's version, `release.sh version`. In a release build that is the
# tag's version, which `release.sh stamp` set for the checkout before
# anything was built (.release-version, TASK_RELEASE_VERSION; Cargo.toml is
# never touched, so a release does not rebuild every workspace crate);
# otherwise it is the committed `[workspace.package] version`. Releases
# commit none.
#
#   scripts/check-versions.sh                 the static checks (no build)
#   scripts/check-versions.sh --bin <path>…   also run each built binary's
#                                             `--version` and compare
#
# The artifacts, and where each one's version comes from:
#
#   task-server (headless node, OCI image payload)  task_release_version (main records
#   task CLI (task-cli)                             TASK_RELEASE_VERSION at build time,
#   fts-files-daemon (files-daemon)                 else the workspace version)
#   desktop app (task-app-desktop) + its bundle     task_release_version; CFBundleShortVersionString
#                                                   from MARKETING_VER, default release.sh version
#   in-tree plugins                                 SHIPS_WITH_TASK: the app's, at runtime
#   iOS app (task-app-mobile), marketing version    MARKETING_VER in task-ios.yml / deploy-testflight.sh
#   its watch app + widget extension, the Finder    $(MARKETING_VERSION)/$(CURRENT_PROJECT_VERSION) in
#   extension: the app's version AND build number   their plists, stamped by the build (scripts/apple-release.sh)
#   web bundle (task-app-web)                       task_release_version, as above
#   OCI images ghcr.io/…/task-{server,web}:<version>  the release tag (images.yml,
#                                                   `release.sh image-tag`)
#   GitHub Release v<version> + node binaries       release.sh publish / node-binaries.yml
#   Nix package task-server                         nix/modules/packages/task-server.nix, from Cargo.toml
#
# Static checks fail when any of those takes its version from anywhere
# else (a literal in a manifest, a hard-coded script default). `--bin`
# catches what the static checks cannot: a binary that, built, reports
# something else. Used by `just ci` (static), the deploy (the server it
# ships) and node-binaries.yml (every release binary).
# shellcheck disable=SC2016 # the $(...) below are literal Xcode placeholders
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

want="$(bash scripts/release.sh version)"
[[ "$want" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "check-versions: workspace version '$want' is not X.Y.Z" >&2; exit 1; }

fail=0
bad() { echo "check-versions: $*" >&2; fail=1; }

# ── Rust artifacts inherit the workspace version ─────────────────────────
for manifest in \
    apps/server/Cargo.toml \
    apps/cli/Cargo.toml \
    apps/desktop/Cargo.toml \
    apps/mobile/Cargo.toml \
    apps/web/Cargo.toml \
    features/files/files-daemon/Cargo.toml; do
    # The [package] table's version line, whatever its spelling.
    line="$(sed -n '/^\[package\]/,/^\[/{/^version/p;}' "$manifest" | head -1)"
    case "$line" in
        "version.workspace = true"|"version = { workspace = true }") ;;
        *) bad "$manifest: [package] version must be \`version.workspace = true\` (it is \`${line:-missing}\`)" ;;
    esac
done

# ── Only binaries read the release version at build time ─────────────────
# crates/release-version says why: a crate that reads TASK_RELEASE_VERSION
# (or a stamped CARGO_PKG_VERSION) recompiles on every release, with
# everything that depends on it.
while IFS= read -r hit; do
    bad "$hit: reads CARGO_PKG_VERSION; report task_release_version::get() (a release would show the committed version)"
done < <(grep -rn --include='*.rs' 'CARGO_PKG_VERSION' apps crates features \
    | grep -v '^crates/release-version/' \
    | grep -vE '^[^:]+:[0-9]+:\s*//' || true)
while IFS= read -r hit; do
    bad "$hit: clap's bare \`version\` reads CARGO_PKG_VERSION; use \`version = task_release_version::get()\`"
done < <(grep -rnE --include='*.rs' '#\[command\([^]]*\bversion\b[,)]' apps crates features || true)
while IFS= read -r hit; do
    bad "$hit: of_this_build! in a library recompiles it on every release; only a binary's main expands it"
done < <(grep -rln --include='*.rs' 'of_this_build!' apps crates features \
    | grep -v '^crates/release-version/' \
    | grep -vE '/src/(main\.rs|bin/[^/]+\.rs)$' || true)
for main in \
    apps/server/src/main.rs \
    apps/cli/src/main.rs \
    apps/desktop/src/main.rs \
    apps/desktop/src/bin/task.rs \
    apps/desktop/src/bin/fts-files-daemon.rs \
    apps/mobile/src/main.rs \
    apps/web/src/main.rs \
    features/files/files-daemon/src/bin/fts-files-daemon.rs; do
    grep -q 'task_release_version::set(task_release_version::of_this_build!())' "$main" \
        || bad "$main: main must record the release version first: task_release_version::set(task_release_version::of_this_build!())"
done

# ── Apple builds default to the release version ──────────────────────────
for script in \
    apps/mobile/ios/deploy-testflight.sh \
    apps/desktop/macos/build-dmg.sh \
    apps/desktop/macos/deploy-testflight-macos.sh; do
    if grep -nE 'MARKETING_VER=.*:-[0-9]' "$script" >/dev/null; then
        bad "$script: MARKETING_VER defaults to a literal version; default it to \`bash scripts/release.sh version\`"
    fi
done
for wf in .github/workflows/task-ios.yml .github/workflows/task-macos.yml; do
    grep -q 'scripts/apple-release.sh version' "$wf" \
        || bad "$wf: the marketing version must come from \`scripts/apple-release.sh version\` (the workspace version)"
    grep -q 'scripts/apple-release.sh build-number' "$wf" \
        || bad "$wf: the build number must come from \`scripts/apple-release.sh build-number\` (one scheme for iOS and macOS)"
    if grep -qE '^\s*marketing_version:' "$wf"; then
        bad "$wf: no marketing_version input; the version is the tag's"
    fi
done
# Embedded bundles (watch app, widget + Finder extensions) take the app's
# versions at build time; App Store Connect rejects any that differ.
for plist in \
    apps/watchos/TaskWatch/Info.plist \
    apps/ios-widgets/TaskWidgets/Info.plist \
    apps/desktop/macos/FileProvider/Info.plist; do
    grep -A1 '<key>CFBundleShortVersionString</key>' "$plist" | grep -qF '$(MARKETING_VERSION)' \
        || bad "$plist: CFBundleShortVersionString must be \$(MARKETING_VERSION) (the app's version, stamped at build)"
    grep -A1 '<key>CFBundleVersion</key>' "$plist" | grep -qF '$(CURRENT_PROJECT_VERSION)' \
        || bad "$plist: CFBundleVersion must be \$(CURRENT_PROJECT_VERSION) (the app's build number, stamped at build)"
done
for project in apps/watchos/project.yml apps/ios-widgets/project.yml; do
    grep -qF 'CFBundleShortVersionString: $(MARKETING_VERSION)' "$project" \
        || bad "$project: info.properties must set CFBundleShortVersionString: \$(MARKETING_VERSION) (xcodegen rewrites the plist)"
done
grep -c 'MARKETING_VERSION="$MARKETING_VER" CURRENT_PROJECT_VERSION="$BUILD_NO"' apps/mobile/ios/deploy-testflight.sh \
    | grep -qx 2 || bad "apps/mobile/ios/deploy-testflight.sh: the watch and widget xcodebuilds must get the app's MARKETING_VERSION and CURRENT_PROJECT_VERSION"

# ── Image tag, release tag, node binaries ────────────────────────────────
# The images are tagged with the release tag's bare version, and every
# release build stamps the tag's version first.
grep -q 'scripts/release.sh image-tag' .github/workflows/images.yml \
    || bad ".github/workflows/images.yml: the image tag must come from \`scripts/release.sh image-tag\` (the release tag)"
for wf in images.yml node-binaries.yml release-assets.yml task-ios.yml task-macos.yml deploy.yml; do
    grep -q 'scripts/release.sh stamp' ".github/workflows/$wf" \
        || bad ".github/workflows/$wf: a release build must run \`scripts/release.sh stamp <tag>\` after its checkout"
done
grep -q 'scripts/release.sh version' .github/workflows/node-binaries.yml \
    || bad ".github/workflows/node-binaries.yml: the release tag must come from \`scripts/release.sh version\`"

# ── Nix ──────────────────────────────────────────────────────────────────
for nixfile in nix/modules/packages/*.nix; do
    if grep -nE '^\s*version\s*=\s*"' "$nixfile" >/dev/null; then
        bad "$nixfile: a literal version; read it from Cargo.toml's [workspace.package]"
    fi
done

# ── Built binaries say it themselves ─────────────────────────────────────
if [ "${1:-}" = "--bin" ]; then
    shift
    [ "$#" -gt 0 ] || bad "--bin needs at least one binary"
    for bin in "$@"; do
        [ -x "$bin" ] || { bad "$bin: not an executable"; continue; }
        # First line, last word: `task-server 0.2.3`, `task 0.2.3`, …
        said="$("$bin" --version 2>/dev/null | head -1 || true)"
        got="${said##* }"
        if [ "$got" != "$want" ]; then
            bad "$bin --version says '${said:-nothing}', want $want"
        else
            echo "check-versions: $bin --version = $said"
        fi
    done
elif [ "$#" -gt 0 ]; then
    echo "usage: scripts/check-versions.sh [--bin <binary>…]" >&2
    exit 2
fi

[ "$fail" = 0 ] || { echo "check-versions: FAILED (workspace version $want)" >&2; exit 1; }
echo "check-versions: every artifact is at $want"
