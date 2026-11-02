{
  description = "Task — local-first vault + tasks/projects/calendar: server, web, desktop, mobile, CLI";

  # Packaging only: the crane builds, the images and
  # `nixosModules.task-server`. The dev environment is devenv
  # (devenv.yaml, devenv.nix; `devenv shell`, or direnv through .envrc).
  #
  # Dendritic layout (den): every .nix under nix/modules/ is a
  # flake-parts module, auto-loaded by import-tree — one file per
  # concern, no central wiring. Shared values flow through the typed
  # `fts.*` perSystem options (nix/modules/options.nix); den's aspect
  # system (nix/modules/den.nix) is the sharing surface with the
  # system flake.
  #
  # The option namespace stays `fts.*` rather than `task.*`: this tree
  # was extracted from the FastTrackStudio monorepo and the two remain
  # sibling repos whose nix modules get diffed and cross-ported. Renaming
  # would touch every module for no functional gain.
  outputs = inputs: inputs.flake-parts.lib.mkFlake { inherit inputs; }
    (inputs.import-tree ./nix/modules);

  inputs = {
    den.url = "github:denful/den";
    import-tree.url = "github:vic/import-tree";

    # The fleet's pins: nixpkgs and rust-overlay at devenv-fleet's
    # pins.json revisions, the same ones devenv.yaml names, with its
    # rust-overlay (`overlays.default`, nix/modules/systems.nix) and
    # `lib.toolchainFor` for the shell's rustc (nix/modules/toolchain.nix).
    # Move them only with devenv-fleet (its `scripts/fleet-pins` checks
    # this lock too).
    devenv-fleet.url = "github:FastTrackStudios/devenv-fleet/v0.1.0";
    nixpkgs.follows = "devenv-fleet/nixpkgs";
    flake-parts.url = "github:hercules-ci/flake-parts";

    # crane — cargo-in-nix builds for the deployable images (task-server
    # + the dx web bundle).
    crane.url = "github:ipetkov/crane";

    # The nixpkgs used ONLY to build `dx` (dioxus-cli), binaryen 129 and
    # wasm-bindgen-cli, at pins.json's `nixpkgs-dx` revision like
    # devenv.yaml's — see nix/modules/dx.nix.
    nixpkgs-dx.url = "github:NixOS/nixpkgs/d99b013d5d1931ad77fe3912ed218170dec5d9a4";
  };

  nixConfig = {
    extra-trusted-public-keys = [
      "fasttrackstudio.cachix.org-1:r7v7WXBeSZ7m5meL6w0wttnvsOltRvTpXeVNItcy9f4="
    ];
    extra-substituters = [
      "https://fasttrackstudio.cachix.org"
    ];
  };
}
