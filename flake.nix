{
  description = "Task — local-first vault + tasks/projects/calendar: server, web, desktop, mobile, CLI";

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

    # Shared Dioxus toolchain hub — every FTS Dioxus repo follows its
    # nixpkgs pin so the package set stays in lockstep.
    dioxus-flake.url = "github:FastTrackStudios/Dioxus-Flake";
    nixpkgs.follows = "dioxus-flake/nixpkgs";

    # rust-overlay is OURS, and deliberately not `dioxus-flake`'s.
    #
    # It followed the hub, on the reasoning that `dx` and rustc should
    # move together. The effect was that the Rust toolchain could only
    # advance when the hub did: rust-overlay sat at 2026-04-05 and
    # pinned us to 1.94.0 while stable reached 1.98.1 five months later,
    # and `rust-toolchain.toml` recorded the consequence as "moving past
    # it means bumping dioxus-flake, not editing this file".
    #
    # The lockstep argument does not actually apply here, because `dx`
    # does not come from the hub either — it is sourced from the
    # dedicated `nixpkgs-dx` pin below, at the version the workspace
    # Cargo.lock wants (see nix/modules/dx.nix). So following the hub
    # for rust-overlay bought no coupling we needed and cost us the
    # ability to take a compiler release.
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-parts.url = "github:hercules-ci/flake-parts";

    # crane — cargo-in-nix builds for the deployable images (task-server
    # + the dx web bundle).
    crane.url = "github:ipetkov/crane";

    # Dedicated, current-unstable nixpkgs used ONLY to source `dx`
    # (dioxus-cli) at the version the workspace Cargo.lock pins, plus
    # binaryen 129 — see nix/modules/dx.nix.
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
