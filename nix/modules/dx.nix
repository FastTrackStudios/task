# The dx toolchain trio the hermetic web bundle builds with (fills fts.dx.*).
#
# The same derivations devenv-fleet's `dioxus` module puts in the dev
# shell (devenv.nix), from devenv-fleet's own recipes on the nixpkgs-dx
# pin, so a `dx serve` locally and a `nix build .#task-webapp` agree:
#
#   dx         the fleet's one dx: the Brahmastra-Labs fork at
#              DioxusLabs/dioxus#5668's branch, which fixes wasm-split's
#              symbol resolution (the published 0.8.0-alpha.0 panics
#              splitting this app; docs/task-webapp.md), with the
#              DX_NO_RUSTC_WRAPPER patch. Its revision is pins.json's `dx`.
#   binaryen   129, the wasm-opt dx expects (older ones SIGABRT).
#   wasm-bindgen-cli  0.2.127, this workspace's Cargo.lock; dx rejects a
#              mismatch. Bump it with the lock, here and in devenv.nix's
#              `fleet.dioxus.wasmBindgen` if that ever leaves the default.
{ inputs, ... }:
{
  perSystem = { system, pkgs, ... }:
    let
      pkgsDx = import inputs.nixpkgs-dx { inherit system; };
      fleet = inputs.devenv-fleet;
    in
    {
      fts.dx.cli = pkgs.callPackage "${fleet}/pkgs/dx.nix" { inherit pkgsDx; };
      fts.dx.binaryen = pkgsDx.binaryen;
      fts.dx.wasmBindgen = pkgs.callPackage "${fleet}/pkgs/wasm-bindgen-cli.nix" {
        inherit pkgsDx;
        version = "0.2.127";
        hash = "sha256-di+qBAdd7pENLiIB9CoZoab+W5xeDoByMREcCGTSzWo=";
        cargoHash = "sha256-FTv2GZIAQs0ePdIZXIXil7JbZ6kIT05VG6vqC1qNFxQ=";
      };
    };
}
