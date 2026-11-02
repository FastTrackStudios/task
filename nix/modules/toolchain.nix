# The Rust toolchain the packages build with (fills fts.rustToolchain).
#
# devenv-fleet's, by the same expression the dev shell (devenv.nix) and
# every fleet repo evaluate: rust 1.99.1 + wasm32, with the iOS and Intel
# targets on darwin. On the same nixpkgs and rust-overlay (flake.nix
# follows devenv-fleet's) it is the same store path, so a crane build, CI
# and a local shell share one rustc. The native build environment lives in
# devenv.nix; a package lists only what its own build needs.
{ inputs, ... }:
{
  perSystem = { pkgs, ... }: {
    fts.rustToolchain = inputs.devenv-fleet.lib.toolchainFor pkgs;
  };
}
