# NixOS module for `task email watch` — long-running IMAP IDLE watcher.
#
# Runs `task email watch` against a Bridge-proxied account; on every
# server-pushed event, invokes `task email sweep` so the curator
# skill picks up the new mail. Restarts on failure.
#
# Usage in your NixOS configuration:
#
#   inputs.task.url = "github:FastTrackStudios/task";
#
#   imports = [ inputs.task.nixosModules.task-email-watcher ];
#
#   services.task-email-watcher = {
#     enable = true;
#     package = inputs.task.packages.${pkgs.system}.task-cli;
#     nextcloud = {
#       url = "https://cloud.starcommand.live";
#       user = "curator";
#       passwordFile = config.sops.secrets."starcommand/selfhost/users/curator/password".path;
#     };
#     imap = {
#       host = "127.0.0.1";
#       port = 1143;
#       user = "agent@fasttrackaudio.com";
#       mailbox = "INBOX";
#       passwordFile = config.sops.secrets."cody/proton/bridge_password".path;
#       caBundle = "/var/lib/nc-mail-trust/ca-bundle.crt";
#     };
#     # Account id in Nextcloud Mail whose inbox we're triaging.
#     nextcloudAccount = 3;
#   };
#
{ config, lib, pkgs, ... }:

let
  cfg = config.services.task-email-watcher;
  inherit (lib) mkEnableOption mkOption mkIf types;
in
{
  options.services.task-email-watcher = {
    enable = mkEnableOption "Task email watcher (IMAP IDLE)";

    package = mkOption {
      type = types.package;
      description = "The task-cli package to use.";
    };

    user = mkOption {
      type = types.str;
      default = "task-watcher";
      description = "System user the service runs as.";
    };

    group = mkOption {
      type = types.str;
      default = "task-watcher";
      description = "System group the service runs as.";
    };

    nextcloud = {
      url = mkOption {
        type = types.str;
        description = "Nextcloud base URL (e.g. https://cloud.example.com).";
      };
      user = mkOption {
        type = types.str;
        description = "Nextcloud user that owns the mail account (e.g. `curator`).";
      };
      passwordFile = mkOption {
        type = types.path;
        description = "Path to a file containing the NC user's password (SOPS-decrypted).";
      };
    };

    imap = {
      host = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "IMAP host — for Bridge, loopback.";
      };
      port = mkOption {
        type = types.port;
        default = 1143;
      };
      user = mkOption {
        type = types.str;
        description = "IMAP username (usually the Proton address).";
      };
      mailbox = mkOption {
        type = types.str;
        default = "INBOX";
      };
      passwordFile = mkOption {
        type = types.path;
        description = "Path to a file containing the Bridge IMAP password.";
      };
      caBundle = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          PEM bundle used to verify the IMAP server cert. Null +
          `insecure = true` disables verification (loopback only).
        '';
      };
      insecure = mkOption {
        type = types.bool;
        default = false;
        description = "Skip TLS peer verification. Only safe on 127.0.0.1.";
      };
    };

    nextcloudAccount = mkOption {
      type = types.int;
      description = "Account id in Nextcloud Mail's oc_mail_accounts table.";
    };

    sweepLimit = mkOption {
      type = types.int;
      default = 50;
      description = "Max messages pulled per sweep invocation.";
    };

    logLevel = mkOption {
      type = types.str;
      default = "info";
    };
  };

  config = mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = "/var/lib/task-watcher";
      createHome = true;
    };
    users.groups.${cfg.group} = { };

    systemd.services.task-email-watcher = {
      description = "Task email watcher — IMAP IDLE → sweep loop";
      after = [ "network-online.target" "protonmail-bridge.service" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      path = [ pkgs.coreutils pkgs.bash ];

      environment = {
        NEXTCLOUD_URL = cfg.nextcloud.url;
        NEXTCLOUD_USER = cfg.nextcloud.user;
        RUST_LOG = "task=${cfg.logLevel}";
        TASK_USER = cfg.nextcloud.user;
      };

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        Restart = "always";
        RestartSec = "10s";
        LoadCredential = [
          "imap_password:${cfg.imap.passwordFile}"
          "nc_password:${cfg.nextcloud.passwordFile}"
        ];
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ "/var/lib/task-watcher" ];
      };

      script = let
        caArg =
          if cfg.imap.caBundle != null
          then "--ca-bundle ${toString cfg.imap.caBundle}"
          else "";
        insecureArg = if cfg.imap.insecure then "--insecure" else "";
      in ''
        set -euo pipefail
        export IMAP_PASSWORD="$(cat "$CREDENTIALS_DIRECTORY/imap_password")"
        export NEXTCLOUD_PASSWORD="$(cat "$CREDENTIALS_DIRECTORY/nc_password")"

        # Stream IDLE events; on each event, fire a sweep. The sweep is
        # idempotent and cheap — it just filters NC Mail's message list.
        ${cfg.package}/bin/task email watch \
          --host ${cfg.imap.host} \
          --port ${toString cfg.imap.port} \
          --user ${lib.escapeShellArg cfg.imap.user} \
          --mailbox ${lib.escapeShellArg cfg.imap.mailbox} \
          ${caArg} ${insecureArg} |
        while IFS= read -r event; do
          echo "event: $event" >&2
          ${cfg.package}/bin/task email sweep \
            --account ${toString cfg.nextcloudAccount} \
            --limit ${toString cfg.sweepLimit} \
            || echo "sweep failed, continuing" >&2
        done
      '';
    };
  };
}
