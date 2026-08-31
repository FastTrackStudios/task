# The dx toolchain trio (fills fts.dx.* and fts.pkgsDx).
#
# Dedicated, current-unstable nixpkgs used ONLY to source `dx`
# (dioxus-cli) plus binaryen 129 (the wasm-opt dx expects). The main
# `nixpkgs` (dioxus-flake's pin) carries dioxus-cli 0.7.4 / binaryen
# 126, which dx rejects / SIGABRTs with.
#
# The SAME trio serves the dev shell and the hermetic web bundle, so a
# `dx serve` locally and a `nix build .#task-webapp` agree.
{ inputs, ... }:
{
  perSystem = { system, lib, ... }:
    let
      pkgsDx = import inputs.nixpkgs-dx { inherit system; };
    in
    {
      fts.pkgsDx = pkgsDx;
      # dx at the version the workspace tracks (dioxus 0.8 line — root
      # Cargo.toml pins the dioxus git rev, Cargo.lock resolves it to
      # 0.8.0-alpha.0). nixpkgs carries 0.7.9; override src onto the
      # published alpha crate. Bump together with the dioxus git rev.
      fts.dx.cli = pkgsDx.dioxus-cli.overrideAttrs (old: rec {
        version = "0.8.0-alpha.0";
        # static.crates.io, NOT fetchCrate: nixpkgs' fetchCrate builds the
        # legacy `crates.io/api/v1/crates/<c>/<v>/download` URL, which now
        # answers 403 to the fetcher. The crate is fine and unyanked — only
        # that endpoint is gone. static.crates.io serves the identical
        # tarball, so the hash below is unchanged. This 403 is what broke
        # EVERY iOS build (the devshell can't even evaluate without dx).
        src = pkgsDx.fetchzip {
          name = "dioxus-cli-${version}";
          url = "https://static.crates.io/crates/dioxus-cli/dioxus-cli-${version}.crate";
          hash = "sha256-gEC5MtvkTBAhv2ChvWPQIx4u/OJ5Qx2sN2+epdcXwSA=";
          extension = "tar.gz";
        };
        cargoDeps = pkgsDx.rustPlatform.fetchCargoVendor {
          inherit src;
          name = "dioxus-cli-${version}-vendor";
          hash = "sha256-znRYZFhWP5PzS6ftcShzNBvRqJXRjnM10OZ+KzUOOsg=";
        };
        # 0.7.9-era patches/checks don't apply to the alpha.
        patches = [ ];
        doCheck = false;
        doInstallCheck = false;
      });
      fts.dx.binaryen = pkgsDx.binaryen;

      # wasm-bindgen-cli matching THIS workspace's Cargo.lock
      # (wasm-bindgen 0.2.127 — one patch ahead of the FTS monorepo's
      # 0.2.126); dx rejects a mismatch. Built through pkgsDx (its
      # fetchCargoVendor pulls from static.crates.io; the older pin's
      # fetcher 403s).
      fts.dx.wasmBindgen = pkgsDx.rustPlatform.buildRustPackage rec {
        pname = "wasm-bindgen-cli";
        version = "0.2.127";
        src = pkgsDx.fetchCrate {
          inherit pname version;
          hash = "sha256-di+qBAdd7pENLiIB9CoZoab+W5xeDoByMREcCGTSzWo=";
        };
        cargoHash = "sha256-FTv2GZIAQs0ePdIZXIXil7JbZ6kIT05VG6vqC1qNFxQ=";
        nativeBuildInputs = [ pkgsDx.pkg-config ];
        # No darwin frameworks: nixpkgs removed the apple_sdk stubs — the
        # default apple-sdk in the darwin stdenv covers Security/CF.
        buildInputs = lib.optionals pkgsDx.stdenv.isLinux [ pkgsDx.openssl ];
        doCheck = false;
      };
    };
}
