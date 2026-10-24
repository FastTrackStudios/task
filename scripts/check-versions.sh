#!/usr/bin/env bash
# One version everywhere: every artifact a release ships reports the
# release's version, `[workspace.package] version` in the root Cargo.toml.
# In a release build that is the tag's version, which `release.sh stamp`
# wrote into the checkout before anything was built; releases commit none.
#
#   scripts/check-versions.sh                 the static checks (no build)
#   scripts/check-versions.sh --bin <path>…   also run each built binary's
#                                             `--version` and compare
#
# The artifacts, and where each one's version comes from:
#
#   task-server (headless node, OCI image payload)  CARGO_PKG_VERSION, version.workspace
#   task CLI (task-cli)                             CARGO_PKG_VERSION, version.workspace
#   fts-files-daemon (files-daemon)                 CARGO_PKG_VERSION, version.workspace
#   desktop app (task-app-desktop) + its bundle     CARGO_PKG_VERSION; CFBundleShortVersionString
#                                                   from MARKETING_VER, default release.sh version
#   iOS app (task-app-mobile), marketing version    MARKETING_VER in task-ios.yml / deploy-testflight.sh
#   its watch app + widget extension, the Finder    $(MARKETING_VERSION)/$(CURRENT_PROJECT_VERSION) in
#   extension: the app's version AND build number   their plists, stamped by the build (scripts/apple-release.sh)
#   web bundle (task-app-web)                       CARGO_PKG_VERSION, version.workspace
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
# release build stamps the tag's version first. (desktop-cache.yml builds
# the pure flake at the tag for cachix, so it does not stamp.)
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
