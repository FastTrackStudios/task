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
      # 0.8.0-alpha.0). nixpkgs carries 0.7.9; override src. Bump
      # together with the dioxus git rev.
      #
      # Built from the dioxus REPO (packages/cli), not the published
      # alpha crate, at DioxusLabs/dioxus#5668's branch: that PR fixes
      # wasm-split-cli's symbol resolution (it matched mangled against
      # demangled names, so nearly every shared function fell out of the
      # call graph and `dx build --wasm-split` died in walrus with
      # `assertion failed: !self.dead.contains(&id)` — dioxus#4769). The
      # branch is upstream main@e1c6342 + two commits, one commit behind
      # the workspace's dioxus rev (f717a8e, the Blitz beta.1 sync,
      # which touches no CLI code). Move back to a published crate once
      # the PR is in a release.
      fts.dx.cli = pkgsDx.dioxus-cli.overrideAttrs (old: rec {
        version = "0.8.0-alpha.0";
        src = pkgsDx.fetchFromGitHub {
          owner = "Brahmastra-Labs";
          repo = "dioxus";
          rev = "19ea84261d3feab7c2015d7b5eab2c8514bc2e5c";
          hash = "sha256-eEbQokzRBTOkHiMloKC85yIpMY8J+rds0ILxZt4oa5I=";
        };
        cargoDeps = pkgsDx.rustPlatform.fetchCargoVendor {
          inherit src;
          name = "dioxus-cli-${version}-vendor";
          hash = "sha256-ySPcW+fE/rcx8zolmKaYvMVKoWvwse68T5qOlIcv5Jk=";
        };
        # The workspace root is the repo; the CLI crate is a member.
        buildAndTestSubdir = "packages/cli";
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
