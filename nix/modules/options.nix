# Shared perSystem option surface — every other module either fills or
# consumes `fts.*`. This is the ONLY cross-module contract; modules never
# reach into each other directly (dendritic rule: files are self-contained
# except for this schema).
#
# The namespace is `fts.*`, not `task.*`, on purpose: this tree was
# extracted from the FastTrackStudio monorepo and the two stay sibling
# repos whose nix modules get diffed and cross-ported. See flake.nix.
{ lib, flake-parts-lib, ... }:
{
  options.perSystem = flake-parts-lib.mkPerSystemOption (_: {
    options.fts = {
      rustToolchain = lib.mkOption {
        type = lib.types.package;
        description = "The fleet's pinned Rust toolchain (devenv-fleet: stable 1.99.1 + wasm32).";
      };
      # dx toolchain trio from the nixpkgs-dx pin — the dev shell and the
      # hermetic web bundle must use the SAME dx/wasm-bindgen/binaryen.
      dx = {
        cli = lib.mkOption { type = lib.types.package; };
        wasmBindgen = lib.mkOption { type = lib.types.package; };
        binaryen = lib.mkOption { type = lib.types.package; };
      };
      # crane build plumbing shared by all deployable packages.
      craneLib = lib.mkOption { type = lib.types.raw; };
      src = lib.mkOption { type = lib.types.raw; };
      commonArgs = lib.mkOption { type = lib.types.raw; };
      buildRev = lib.mkOption {
        type = lib.types.str;
        description = "Git rev baked into deployable images (version.json / TASK_BUILD_REV).";
      };
      mkStaticSite = lib.mkOption {
        type = lib.types.raw;
        description = "static-web-server OCI image factory: { name, tag ?, siteRoot }.";
      };
      imageLabels = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        description = "OCI labels on every published image (source repo, revision, version).";
      };
      mkServerImage = lib.mkOption {
        type = lib.types.raw;
        description = "task-server OCI image factory: { server } — the package holding bin/task-server.";
      };
    };
  });
}
