# cargo-rail — CI change selection (`cargo rail plan`): which workspace
# packages a change actually affects, through cargo's resolved
# reverse-dependency graph. Task is a ~163-member workspace, so running
# only the affected tests and clippy is worth real minutes per PR.
# nixpkgs carries 0.7.0, which predates the plan contract, so it is built
# here from the release tag.
#
# Built with the workspace toolchain (fts.rustToolchain), because rail's
# rust-version (1.98.1) is newer than any stable nixpkgs carries. The
# resolve-graph patch this
# module used to carry for 0.17.3 is gone: upstream keys graph nodes by
# exact PackageId and builds edges from cargo's resolved graph since the
# 0.2x line, which is what the patch did.
#
# Only `plan` (and `config`) are used. rail's compiler cache (`cache
# setup` installs a global cargo wrapper) stays off: our runners keep a
# warm target dir, and dx interposes its own rustc wrapper. `release` is
# disabled in .config/rail.toml; releases go through scripts/release.sh.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      rustPlatform = pkgs.makeRustPlatform {
        cargo = config.fts.rustToolchain;
        rustc = config.fts.rustToolchain;
      };
      cargo-rail = rustPlatform.buildRustPackage rec {
        pname = "cargo-rail";
        version = "0.32.0";
        src = pkgs.fetchFromGitHub {
          owner = "loadingalias";
          repo = "cargo-rail";
          rev = "v${version}";
          hash = "sha256-XdwwQVv3OIauYefmQNq6CKK/fcaEnmq6Rkilmg9bhk8=";
        };
        cargoHash = "sha256-kpFJ3/s1qgXqY7Nhci7/4YtMcw7E+UmB3SX8F5MHoxs=";
        doCheck = false;
        meta = {
          description = "Cargo-native monorepo change selection (plan), unify, release";
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
