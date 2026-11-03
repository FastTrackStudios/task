# Target systems + the package set: devenv-fleet's nixpkgs pin with its
# rust-overlay, the same pair the dev shell evaluates (devenv.yaml).
{ inputs, lib, ... }:
{
  # mkForce: den's flakeModule contributes nix's full default-systems
  # list; without the override the merge would fan every output out to
  # ten platforms and slow eval/CI for targets we never build. No
  # x86_64-darwin: the fleet's nixpkgs (26.11) dropped it. Intel Macs
  # still get a binary, cross-built from aarch64-darwin (the toolchain's
  # x86_64-apple-darwin target).
  systems = lib.mkForce [ "x86_64-linux" "aarch64-darwin" "aarch64-linux" ];

  perSystem = { system, ... }: {
    _module.args.pkgs = import inputs.nixpkgs {
      inherit system;
      overlays = [ inputs.devenv-fleet.overlays.default ];
      config.allowUnfree = true;
    };
  };
}
