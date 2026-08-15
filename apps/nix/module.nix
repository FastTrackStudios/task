# NixOS module for task-server
#
# Usage in your NixOS configuration:
#
#   # flake.nix inputs:
#   inputs.task.url = "github:FastTrackStudios/task";
#
#   # configuration.nix:
#   imports = [ inputs.task.nixosModules.task-server ];
#
#   services.task-server = {
#     enable = true;
#     vaultRoot = "/mnt/starcommand/Projects";
#     serverName = "starcommand";
#     port = 3456;
#     nextcloud = {
#       enable = true;
#       url = "https://cloud.starcommand.live";
#       username = "codywright";
#       passwordFile = config.sops.secrets."task/nextcloud-password".path;
#       projectsPath = "Projects/";
#       calendar = "Personal";
#       deckEnabled = true;
#     };
#   };
#
{ config, lib, pkgs, ... }:

let
  cfg = config.services.task-server;
  inherit (lib) mkEnableOption mkOption mkIf types;
in
{
  options.services.task-server = {
    enable = mkEnableOption "Task management server";

    package = mkOption {
      type = types.package;
      description = "The task-server package to use.";
    };

    # ── Server identity ───────────────────────────────────────────────

    serverName = mkOption {
      type = types.str;
      default = config.networking.hostName;
      description = "Human-readable name for this server instance.";
    };

    serverId = mkOption {
      type = types.str;
      default = "${config.networking.hostName}-task";
      description = "Unique stable identifier for this server.";
    };

    # ── Network ───────────────────────────────────────────────────────

    port = mkOption {
      type = types.port;
      default = 3456;
      description = "Port to listen on.";
    };

    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address to bind to.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Whether to open the port in the firewall.";
    };

    # ── Vault configuration ───────────────────────────────────────────

    vaultRoot = mkOption {
      type = types.path;
      description = ''
        Root directory for projects. This is where the server reads
        project.md + tasks/*.md files.

        Can be a local path, NFS mount, or Nextcloud sync directory.
      '';
    };

    databasePath = mkOption {
      type = types.path;
      default = "/var/lib/task-server/task.sqlite";
      description = "SQLite database path for auth, organizations, activity, and other authoritative service data.";
    };

    publicBaseUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Public base URL used by auth callbacks. Defaults to the bind address.";
    };

    seedDemo = mkOption {
      type = types.bool;
      default = true;
      description = "Seed demo users, organizations, projects, and tasks on startup. Disable for production instances.";
    };

    bootstrapAuth = {
      enable = mkEnableOption "bootstrap a production auth user, organization, membership, and session token";

      sessionTokenFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to a file containing the production Task session token.
          Use a secret manager such as sops-nix; the token is inserted into
          auth_sessions at service startup and is never placed in the Nix store.
        '';
      };

      userId = mkOption {
        type = types.str;
        default = "user_cody";
        description = "Stable auth user id to create for the bootstrap session.";
      };

      email = mkOption {
        type = types.str;
        default = "cody@fasttrackstudio.com";
        description = "Bootstrap user email.";
      };

      name = mkOption {
        type = types.str;
        default = "Cody Wright";
        description = "Bootstrap user display name.";
      };

      username = mkOption {
        type = types.str;
        default = "cody";
        description = "Bootstrap username.";
      };

      organizationId = mkOption {
        type = types.str;
        default = "org_fts";
        description = "Stable auth organization id to create.";
      };

      organizationName = mkOption {
        type = types.str;
        default = "Fast Track Studio";
        description = "Bootstrap organization name.";
      };

      organizationSlug = mkOption {
        type = types.str;
        default = "fast-track-studio";
        description = "Bootstrap organization slug.";
      };

      sessionId = mkOption {
        type = types.str;
        default = "session_cody_bootstrap";
        description = "Stable auth session id for the bootstrap token.";
      };
    };

    # ── Nextcloud integration ─────────────────────────────────────────

    nextcloud = {
      enable = mkEnableOption "Nextcloud integration";

      url = mkOption {
        type = types.str;
        default = "";
        description = "Nextcloud base URL (e.g. https://cloud.example.com).";
      };

      username = mkOption {
        type = types.str;
        default = "";
        description = "Nextcloud username.";
      };

      passwordFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to a file containing the Nextcloud app password.
          The file should contain just the password, no newline.
          Use with sops-nix or agenix for secrets management.
        '';
      };

      projectsPath = mkOption {
        type = types.str;
        default = "Projects/";
        description = "Path within the user's Nextcloud files to the Projects directory.";
      };

      calendar = mkOption {
        type = types.str;
        default = "Personal";
        description = "CalDAV calendar name for task sync.";
      };

      eventCalendar = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Optional CalDAV calendar name for non-task events.";
      };

      deckEnabled = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to sync with Nextcloud Deck boards.";
      };
    };

    nextcloudVaultAcl = {
      enable = mkEnableOption ''
        POSIX ACL management for a Nextcloud-owned vault tree.

        Enable this when vaultRoot points at live Nextcloud storage such as
        /var/lib/nextcloud/data/<user>/files/Projects and task-server needs
        write access without changing Nextcloud ownership.
      '';

      path = mkOption {
        type = types.path;
        default = cfg.vaultRoot;
        defaultText = lib.literalExpression "config.services.task-server.vaultRoot";
        description = ''
          Directory that should receive ACLs for the task-server user.
          Defaults to vaultRoot.
        '';
      };

      recursive = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Apply the access ACL to existing files/directories under path and
          apply default ACLs to existing directories so new files inherit
          task-server write access.
        '';
      };
    };

    # ── CalDAV sync ───────────────────────────────────────────────────

    caldav = {
      enable = mkEnableOption "CalDAV task sync (VTODO)";

      serverUrl = mkOption {
        type = types.str;
        default = "";
        description = "CalDAV server URL.";
      };

      username = mkOption {
        type = types.str;
        default = "";
        description = "CalDAV username.";
      };

      passwordFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = "Path to file containing CalDAV password.";
      };

      calendarPath = mkOption {
        type = types.str;
        default = "calendars/user/tasks/";
        description = "Calendar collection path on the CalDAV server.";
      };

      cacheDir = mkOption {
        type = types.path;
        default = "/var/lib/task-server/caldav-cache";
        description = "Local cache directory for .ics files.";
      };
    };

    # ── Logging ───────────────────────────────────────────────────────

    logLevel = mkOption {
      type = types.str;
      default = "info";
      description = "Log level (trace, debug, info, warn, error).";
    };

    # ── User/group ────────────────────────────────────────────────────

    user = mkOption {
      type = types.str;
      default = "task-server";
      description = "User to run the server as.";
    };

    group = mkOption {
      type = types.str;
      default = "task-server";
      description = "Group to run the server as.";
    };
  };

  config = mkIf cfg.enable {
    # Create system user and group
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = "/var/lib/task-server";
      createHome = true;
    };

    users.groups.${cfg.group} = {};

    system.activationScripts.task-server-nextcloud-vault-acl = mkIf cfg.nextcloudVaultAcl.enable {
      deps = [ "users" "groups" ];
      text =
        let
          aclRoot = toString cfg.nextcloudVaultAcl.path;
          setfacl = "${pkgs.acl}/bin/setfacl";
          find = "${pkgs.findutils}/bin/find";
        in
        ''
          if [ -d ${lib.escapeShellArg aclRoot} ]; then
            ${if cfg.nextcloudVaultAcl.recursive then ''
              ${setfacl} -R -m ${lib.escapeShellArg "u:${cfg.user}:rwX"} ${lib.escapeShellArg aclRoot}
              ${find} ${lib.escapeShellArg aclRoot} -type d -exec \
                ${setfacl} -m ${lib.escapeShellArg "d:u:${cfg.user}:rwX"} {} +
            '' else ''
              ${setfacl} -m ${lib.escapeShellArg "u:${cfg.user}:rwX"} ${lib.escapeShellArg aclRoot}
              ${setfacl} -m ${lib.escapeShellArg "d:u:${cfg.user}:rwX"} ${lib.escapeShellArg aclRoot}
            ''}
          else
            echo "task-server: nextcloudVaultAcl path does not exist, skipping ACL setup: ${aclRoot}" >&2
          fi
        '';
    };

    # Firewall
    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ cfg.port ];

    # Systemd service
    systemd.services.task-server = {
      description = "Task Management Server";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        TASK_VAULT = toString cfg.vaultRoot;
        VAULT_ROOT = toString cfg.vaultRoot;
        TASK_DB_PATH = toString cfg.databasePath;
        TASK_SEED_DEMO = if cfg.seedDemo then "1" else "0";
        SERVER_NAME = cfg.serverName;
        SERVER_ID = cfg.serverId;
        BIND_ADDR = "${cfg.bindAddress}:${toString cfg.port}";
        RUST_LOG = "task_server=${cfg.logLevel}";
      } // lib.optionalAttrs (cfg.publicBaseUrl != null) {
        PUBLIC_BASE_URL = cfg.publicBaseUrl;
      } // lib.optionalAttrs cfg.nextcloud.enable {
        NEXTCLOUD_URL = cfg.nextcloud.url;
        NEXTCLOUD_USERNAME = cfg.nextcloud.username;
        NEXTCLOUD_PROJECTS_PATH = cfg.nextcloud.projectsPath;
        NEXTCLOUD_CALENDAR = cfg.nextcloud.calendar;
        NEXTCLOUD_DECK_ENABLED = if cfg.nextcloud.deckEnabled then "1" else "0";
      } // lib.optionalAttrs (cfg.nextcloud.enable && cfg.nextcloud.eventCalendar != null) {
        NEXTCLOUD_EVENT_CALENDAR = cfg.nextcloud.eventCalendar;
      } // lib.optionalAttrs cfg.caldav.enable {
        CALDAV_URL = cfg.caldav.serverUrl;
        CALDAV_USERNAME = cfg.caldav.username;
        CALDAV_CALENDAR_PATH = cfg.caldav.calendarPath;
        CALDAV_CACHE_DIR = cfg.caldav.cacheDir;
      };

      script = let
        ncPw = lib.optionalString (cfg.nextcloud.enable && cfg.nextcloud.passwordFile != null) ''
          if [ -r "$CREDENTIALS_DIRECTORY/nextcloud-password" ]; then
            export NEXTCLOUD_PASSWORD="$(< "$CREDENTIALS_DIRECTORY/nextcloud-password")"
          fi
        '';
        cdPw = lib.optionalString (cfg.caldav.enable && cfg.caldav.passwordFile != null) ''
          if [ -r "$CREDENTIALS_DIRECTORY/caldav-password" ]; then
            export CALDAV_PASSWORD="$(< "$CREDENTIALS_DIRECTORY/caldav-password")"
          fi
        '';
        authBootstrap = lib.optionalString cfg.bootstrapAuth.enable ''
          if [ -r "$CREDENTIALS_DIRECTORY/session-token" ] && [ -e "$TASK_DB_PATH" ]; then
            token="$(< "$CREDENTIALS_DIRECTORY/session-token")"
            now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
            expires="$(date -u -d '+365 days' +%Y-%m-%dT%H:%M:%SZ)"
            ${pkgs.sqlite}/bin/sqlite3 "$TASK_DB_PATH" <<SQL
INSERT INTO auth_users (id, email, name, email_verified, created_at, updated_at, metadata, username, display_username, two_factor_enabled, banned)
VALUES ('${cfg.bootstrapAuth.userId}', '${cfg.bootstrapAuth.email}', '${cfg.bootstrapAuth.name}', 1, '$now', '$now', '{}', '${cfg.bootstrapAuth.username}', '${cfg.bootstrapAuth.username}', 0, 0)
ON CONFLICT(id) DO UPDATE SET email=excluded.email, name=excluded.name, updated_at=excluded.updated_at, username=excluded.username, display_username=excluded.display_username;
INSERT INTO auth_organizations (id, name, slug, metadata, created_at, updated_at)
VALUES ('${cfg.bootstrapAuth.organizationId}', '${cfg.bootstrapAuth.organizationName}', '${cfg.bootstrapAuth.organizationSlug}', '{}', '$now', '$now')
ON CONFLICT(id) DO UPDATE SET name=excluded.name, slug=excluded.slug, updated_at=excluded.updated_at;
INSERT INTO auth_members (id, organization_id, user_id, role, created_at)
VALUES ('member_${cfg.bootstrapAuth.userId}_${cfg.bootstrapAuth.organizationId}', '${cfg.bootstrapAuth.organizationId}', '${cfg.bootstrapAuth.userId}', 'owner', '$now')
ON CONFLICT(id) DO UPDATE SET role=excluded.role;
INSERT INTO auth_sessions (id, expires_at, token, created_at, updated_at, user_id, active_organization_id, active)
VALUES ('${cfg.bootstrapAuth.sessionId}', '$expires', '$token', '$now', '$now', '${cfg.bootstrapAuth.userId}', '${cfg.bootstrapAuth.organizationId}', 1)
ON CONFLICT(id) DO UPDATE SET expires_at=excluded.expires_at, token=excluded.token, updated_at=excluded.updated_at, user_id=excluded.user_id, active_organization_id=excluded.active_organization_id, active=1;
SQL
          fi
        '';
      in ''
        ${ncPw}
        ${cdPw}
        ${authBootstrap}
        exec ${cfg.package}/bin/task-server
      '';

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        # Security hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ReadWritePaths = [
          (toString cfg.vaultRoot)
          "/var/lib/task-server"
          (toString (dirOf cfg.databasePath))
        ] ++ lib.optionals cfg.caldav.enable [
          cfg.caldav.cacheDir
        ];

        # Load secrets from files
        LoadCredential = lib.optional (cfg.nextcloud.enable && cfg.nextcloud.passwordFile != null)
          "nextcloud-password:${cfg.nextcloud.passwordFile}"
        ++ lib.optional (cfg.caldav.enable && cfg.caldav.passwordFile != null)
          "caldav-password:${cfg.caldav.passwordFile}"
        ++ lib.optional (cfg.bootstrapAuth.enable && cfg.bootstrapAuth.sessionTokenFile != null)
          "session-token:${cfg.bootstrapAuth.sessionTokenFile}";
      };
    };

    # Create cache directory
    systemd.tmpfiles.rules = [
      "d ${dirOf cfg.databasePath} 0750 ${cfg.user} ${cfg.group} -"
    ] ++ lib.optionals cfg.caldav.enable [
      "d ${cfg.caldav.cacheDir} 0750 ${cfg.user} ${cfg.group} -"
    ];
  };
}
