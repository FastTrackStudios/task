# NixOS module: an always-on, headless Task node.
#
# Task has no central server (docs/spec/offline.md, `offline.peers`): every
# install is a node, and `task-server` is the one with no window. This
# module runs it as a system service, by default as `task-server peer` —
# one more of your devices that simply stays on — or, with
# `mode = "host"`, hosting the orgs under its data root the way the
# cluster pod does.
#
# The flake exports it (nix/modules/nixos-modules.nix) with `package`
# defaulting to the same flake's `task-server`, so pinning the flake
# input to a release tag pins the node to that release:
#
#   # nix-fleet flake.nix
#   inputs.task.url = "github:FastTrackStudios/task/v0.2.3";
#
#   # a host
#   imports = [ inputs.task.nixosModules.task-server ];
#   services.task-server = {
#     enable = true;
#     openFirewall = true;
#   };
#
# Then, once, sign the node in as your account (a code to approve on
# your phone):
#
#   sudo task-server-node peer sign-in
#
# Updates: move the `task` input to the new tag (the fleet's release
# follower does it) and switch; the unit's ExecStart changes with the
# package, so the node restarts on the new release. See
# docs/headless-node.md.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.task-server;
  inherit (lib) mkEnableOption mkOption mkIf types;

  # The node's own command line.
  nodeArgs = lib.optionals (cfg.mode == "peer") [ "peer" ];

  env = {
    TASK_DATA_ROOT = toString cfg.dataDir;
    TASK_SERVER_BIND = "${cfg.bindAddress}:${toString cfg.port}";
    RUST_LOG = "task_server=${cfg.logLevel}";
  }
  // lib.optionalAttrs (cfg.peerName != null) { TASK_PEER_NAME = cfg.peerName; }
  // lib.optionalAttrs (cfg.mode == "host" && cfg.org != null) { TASK_SERVER_ORG = cfg.org; }
  // cfg.environment;

  # `task-server` as the service's user, against the service's data root —
  # what `peer sign-in`, `peer status` and `admin …` need to act on THIS
  # node rather than on ./data in the caller's directory.
  nodeCli = pkgs.writeShellScriptBin "task-server-node" ''
    exec ${pkgs.util-linux}/bin/runuser -u ${lib.escapeShellArg cfg.user} -- \
      ${pkgs.coreutils}/bin/env ${lib.concatStringsSep " " (lib.mapAttrsToList (k: v: lib.escapeShellArg "${k}=${v}") env)} \
      ${lib.getExe' cfg.package "task-server"} "$@"
  '';

  stateDirName = lib.removePrefix "/var/lib/" (toString cfg.dataDir);
  usesStateDir = lib.hasPrefix "/var/lib/" (toString cfg.dataDir) && stateDirName != "";
in
{
  options.services.task-server = {
    enable = mkEnableOption "an always-on, headless Task node (task-server)";

    package = mkOption {
      type = types.package;
      description = ''
        The task-server package. The flake's exported module defaults this
        to the same flake's `task-server`, so the node follows the flake pin.
      '';
    };

    mode = mkOption {
      type = types.enum [ "peer" "host" ];
      default = "peer";
      description = ''
        `peer`: run as one more of the account's devices (`task-server peer`):
        it joins the account's shared org from a device that holds it, admits
        the account's other devices and is admitted by them. Sign it in once
        with `task-server-node peer sign-in`.

        `host`: host the orgs under {option}`dataDir` (a bare `task-server`,
        the cluster pod's mode).
      '';
    };

    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/task-server";
      description = ''
        `TASK_DATA_ROOT`: the node's data (orgs, iroh identity, the account's
        session). Under /var/lib it is a systemd StateDirectory; elsewhere it
        is created by tmpfiles.
      '';
    };

    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address the node serves on (`TASK_SERVER_BIND`).";
    };

    port = mkOption {
      type = types.port;
      default = 9090;
      description = "Port the node serves on (`TASK_SERVER_BIND`).";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Open {option}`port` (TCP). Device-to-device sync runs over iroh
        (QUIC, UDP) and finds its way through NAT without this.
      '';
    };

    peerName = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "battleship-node";
      description = "The name the account's other devices see (`TASK_PEER_NAME`; default: the hostname).";
    };

    org = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Host mode: the org slug to serve (`TASK_SERVER_ORG`).";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = { TASK_DEVICE_SYNC_SECS = "60"; };
      description = "Extra environment for the node (non-secret; see .env.example).";
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "A file of KEY=value lines for secrets (systemd EnvironmentFile), e.g. from sops-nix.";
    };

    logLevel = mkOption {
      type = types.str;
      default = "info";
      description = "`RUST_LOG` level for task_server.";
    };

    user = mkOption {
      type = types.str;
      default = "task-server";
      description = "User the node runs as.";
    };

    group = mkOption {
      type = types.str;
      default = "task-server";
      description = "Group the node runs as.";
    };
  };

  config = mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = toString cfg.dataDir;
    };
    users.groups.${cfg.group} = { };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ cfg.port ];

    # `task-server --version` for anyone; `task-server-node …` to act on
    # this node (sign-in, status, admin) as its user.
    environment.systemPackages = [ cfg.package nodeCli ];

    # Also under /var/lib: `task-server-node peer sign-in` runs before the
    # service has ever started, and must find the directory it writes to.
    systemd.tmpfiles.rules = [
      "d ${toString cfg.dataDir} 0700 ${cfg.user} ${cfg.group} -"
    ];

    systemd.services.task-server = {
      description = "Task node (headless)";
      documentation = [ "https://github.com/FastTrackStudios/task/blob/main/docs/headless-node.md" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      environment = env;
      # git: the snapshot engine and repo-sourced wikis shell out to it.
      path = [ pkgs.git ];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        ExecStart = lib.escapeShellArgs ([ (lib.getExe' cfg.package "task-server") ] ++ nodeArgs);
        Restart = "on-failure";
        RestartSec = "5s";
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        ReadWritePaths = [ (toString cfg.dataDir) ];
      }
      // lib.optionalAttrs usesStateDir {
        StateDirectory = stateDirName;
        StateDirectoryMode = "0700";
      }
      // lib.optionalAttrs (cfg.environmentFile != null) {
        EnvironmentFile = cfg.environmentFile;
      };
    };
  };
}
