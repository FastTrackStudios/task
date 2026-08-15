# cargo-rail — graph-aware workspace maintenance (unify / plan / run /
# change / release / split). This is the module that earns its keep here:
# Task is a ~163-member workspace, so CI change-selection (`cargo rail
# plan`) is worth real minutes per PR. Built from source (nixpkgs carries
# 0.7.0, which predates the plan/change/release surface) with one patch:
#
#   nix/patches/cargo-rail-resolve-graph.patch — build dependency edges
#   from cargo's RESOLVE graph (package-ID-keyed) instead of manifest
#   dep names. Upstream v0.17.3 keys nodes by bare name over ALL
#   packages, so a third-party crate sharing a workspace member's name
#   (or a git copy of a workspace crate — this tree has several, since
#   `daw`, `session`, `architect` … arrive as git deps) welds the graph
#   together and every leaf change "affects" everything. Candidate for
#   upstreaming.
#
# rust-version is relaxed 1.95 → 1.94: rail's floor was a policy bump,
# not a feature need, and the crate compiles fine on 1.94.
#
# Going to 1.96 would drop this patch (and the kstring 2.0.2 pin in
# Cargo.toml), but it is not free: rust-overlay follows dioxus-flake,
# whose newest stable is 1.94.1, so upgrading means un-following the hub
# that keeps `dx` and rustc in lockstep. Bump dioxus-flake first, then
# both workarounds can go.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      pkgsDx = config.fts.pkgsDx;
      cargo-rail = pkgsDx.rustPlatform.buildRustPackage rec {
        pname = "cargo-rail";
        version = "0.17.3";
        src = pkgsDx.fetchFromGitHub {
          owner = "loadingalias";
          repo = "cargo-rail";
          rev = "v${version}";
          hash = "sha256-ROro3gyTGo38yzRi7c8UUsCZ1HqJVUD8pCYauh1H10s=";
        };
        cargoHash = "sha256-OuSc0bmXmdh2ZeENz1Ecbw0ALTX+HTv7XMiMy3iFcko=";
        patches = [ ../patches/cargo-rail-resolve-graph.patch ];
        postPatch = ''
          substituteInPlace Cargo.toml \
            --replace-fail 'rust-version = "1.95.0"' 'rust-version = "1.94.0"'
        '';
        doCheck = false;
        meta = {
          description = "Cargo-native monorepo control plane: unify, plan/run, change/release, split/sync";
          homepage = "https://github.com/loadingalias/cargo-rail";
          license = lib.licenses.mit;
          mainProgram = "cargo-rail";
        };
      };
    in
    {
      fts.cargoRail = cargo-rail;
    };
}
