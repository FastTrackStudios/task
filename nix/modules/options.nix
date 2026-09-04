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
        description = "FTS-wide pinned Rust toolchain (stable 1.94 + wasm32).";
      };
      buildInputs = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        description = "Native libraries every cargo build in the tree links against.";
      };
      nativeBuildInputs = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        description = "Host tools for interactive builds (pkg-config, bindgen, dx…).";
      };
      shellEnv = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        description = "Env every dev/CI shell exports (bindgen, wasm CC, LD paths…).";
      };
      # dx toolchain trio from the nixpkgs-dx pin — shells and the
      # hermetic web bundle must use the SAME dx/wasm-bindgen/binaryen.
      dx = {
        cli = lib.mkOption { type = lib.types.package; };
        wasmBindgen = lib.mkOption { type = lib.types.package; };
        binaryen = lib.mkOption { type = lib.types.package; };
      };
      pkgsDx = lib.mkOption {
        type = lib.types.raw;
        description = "The nixpkgs-dx package set (current-unstable).";
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
      mkServerImage = lib.mkOption {
        type = lib.types.raw;
        description = "task-server OCI image factory: { server } — the package holding bin/task-server.";
      };
      cargoRail = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        description = "cargo-rail release binary (null where upstream ships no asset).";
      };
    };
  });
}
