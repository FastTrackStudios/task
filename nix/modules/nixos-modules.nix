# NixOS modules this flake exports, for nix-fleet (or any NixOS flake) to
# import:
#
#   imports = [ inputs.task.nixosModules.task-server ];
#   services.task-server.enable = true;
#
# `task-server` is the headless node (nix/nixos/task-server.nix). Its
# `package` defaults to THIS flake's `task-server`, built for the host's
# platform — so the release a host runs is exactly the tag its `task`
# input is pinned to, and following releases is moving that pin
# (docs/headless-node.md, "Updates").
#
# A plain `flake.nixosModules` output rather than a den aspect: every
# NixOS flake can import it, and a den-based fleet wraps it in one line
# (`den.aspects.task-node.nixos = inputs.task.nixosModules.task-server;`).
{ self, lib, inputs, ... }:
let
  taskServer = { pkgs, ... }: {
    imports = [ ../nixos/task-server.nix ];
    services.task-server.package = lib.mkDefault
      self.packages.${pkgs.stdenv.hostPlatform.system}.task-server;
  };
in
{
  flake.nixosModules = {
    task-server = taskServer;
    default = taskServer;
  };

  # The module evaluates and renders the unit the docs describe. Cheap: it
  # evaluates a container config with a stand-in package (no cargo build)
  # and checks the systemd unit it produces. `nix build .#checks.<sys>.nixos-task-server`.
  perSystem = { pkgs, system, ... }: {
    checks = lib.optionalAttrs pkgs.stdenv.isLinux {
      nixos-task-server =
        let
          stub = pkgs.writeShellScriptBin "task-server" "echo task-server 0.0.0";
          eval = inputs.nixpkgs.lib.nixosSystem {
            inherit system;
            modules = [
              self.nixosModules.task-server
              {
                boot.isContainer = true;
                system.stateVersion = "25.05";
                services.task-server = {
                  enable = true;
                  package = stub;
                  peerName = "test-node";
                };
              }
            ];
          };
          unit = eval.config.systemd.units."task-server.service".text;
          # hasInfix is a regex match, which refuses strings that carry
          # store-path context; compare the text only.
          plain = builtins.unsafeDiscardStringContext;
          want = [
            "ExecStart=${stub}/bin/task-server peer"
            "Environment=\"TASK_DATA_ROOT=/var/lib/task-server\""
            "Environment=\"TASK_SERVER_BIND=0.0.0.0:9090\""
            "Environment=\"TASK_PEER_NAME=test-node\""
            "StateDirectory=task-server"
            "User=task-server"
          ];
          missing = lib.filter (w: !(lib.hasInfix (plain w) (plain unit))) want;
        in
        assert lib.assertMsg (missing == [ ])
          "nixos-task-server: the unit lacks ${lib.concatStringsSep ", " missing}:\n${unit}";
        pkgs.writeText "task-server.service" unit;
    };
  };
}
