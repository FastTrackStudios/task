#!/usr/bin/env bash
# Vendor the vox TypeScript runtime packages out of the cargo git
# checkout into ui-lab/vendor/.
#
# Why vendor instead of pnpm file:-linking into ~/.cargo? The checkout
# path embeds a content hash and gets replaced whenever the workspace
# bumps its vox rev — links rot silently. A copy pinned to the rev in
# Cargo.lock is boring and reproducible. See VENDOR.md.
#
# Usage:
#   ./scripts/vendor-vox.sh [path-to-vox-checkout]
#
# The default path is derived from the rev recorded in VENDOR.md /
# Cargo.lock; pass the checkout explicitly after a vox bump:
#   ./scripts/vendor-vox.sh ~/.cargo/git/checkouts/vox-<hash>/<rev7>
set -euo pipefail

UI_LAB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${1:-/run/media/Development/vox}"
PKGS=(vox-postcard vox-wire vox-core vox-ws)

if [[ ! -d "$SRC/typescript/packages" ]]; then
  echo "error: $SRC does not look like a vox checkout (no typescript/packages)" >&2
  exit 1
fi

for pkg in "${PKGS[@]}"; do
  src_pkg="$SRC/typescript/packages/$pkg"
  dst_pkg="$UI_LAB_DIR/vendor/$pkg"
  rm -rf "$dst_pkg"
  mkdir -p "$dst_pkg"
  cp -r "$src_pkg/src" "$dst_pkg/src"
  cp "$src_pkg/package.json" "$dst_pkg/package.json"
  [[ -f "$src_pkg/README.md" ]] && cp "$src_pkg/README.md" "$dst_pkg/README.md"
  # Tests need vitest (a devDependency we deliberately don't install).
  find "$dst_pkg/src" -name '*.test.ts' -delete
done

# Prune the vendored package.json files:
#  - drop scripts/devDependencies/publishConfig (we consume src directly
#    through the `exports: { ".": "./src/index.ts" }` entry; no build,
#    no vitest/tsdown/@types/node install)
#  - drop vox-ws's dependency on vox-tcp: its runtime (src/index.ts ->
#    src/transport.ts) never imports it (node-only TCP transport), and
#    vendoring a node:net package into a browser app just to satisfy a
#    workspace: range is noise.
node - "$UI_LAB_DIR" <<'EOF'
const fs = require("fs");
const path = require("path");
const uiLab = process.argv[2];
for (const pkg of ["vox-postcard", "vox-wire", "vox-core", "vox-ws"]) {
  const file = path.join(uiLab, "vendor", pkg, "package.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  delete json.scripts;
  delete json.devDependencies;
  delete json.publishConfig;
  delete json.files;
  if (pkg === "vox-ws" && json.dependencies) {
    delete json.dependencies["@bearcove/vox-tcp"];
  }
  fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
}
EOF

echo "Vendored ${PKGS[*]} from $SRC"
echo "Remember to update the rev recorded in vendor/VENDOR.md if it changed."
