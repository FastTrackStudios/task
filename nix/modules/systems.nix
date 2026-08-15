# Target systems + the package set: dioxus-flake's nixpkgs pin with the
# rust-overlay (toolchain lockstep across every FTS Dioxus repo).
{ inputs, lib, ... }:
{
  # mkForce: den's flakeModule contributes nix's full default-systems
  # list; without the override the merge would fan every output out to
  # ten platforms and slow eval/CI for targets we never build.
  systems = lib.mkForce [ "x86_64-linux" "x86_64-darwin" "aarch64-darwin" "aarch64-linux" ];

  perSystem = { system, ... }: {
    _module.args.pkgs = import inputs.nixpkgs {
      inherit system;
      overlays = [ inputs.rust-overlay.overlays.default ];
      config.allowUnfree = true;
    };
  };
}
