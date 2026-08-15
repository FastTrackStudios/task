# task-server, built from the workspace in ONE derivation
# (cargoArtifacts = null skips crane's deps-only split — mkDummySrc over
# a ~163-member workspace with custom build.rs files is not worth the
# fragility). Plus its OCI image (Linux-only).
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      task-server = config.fts.craneLib.buildPackage (config.fts.commonArgs // {
        pname = "task-server";
        version = "0.1.0";
        cargoArtifacts = null;
        cargoExtraArgs = "--package task-server";
        doCheck = false;
      });
    in
    {
      packages = { inherit task-server; }
      // lib.optionalAttrs pkgs.stdenv.isLinux {
        task-server-image = pkgs.dockerTools.streamLayeredImage {
          name = "task-server";
          tag = "latest";
          # git + curl: the snapshot engine shells out to them; cacert
          # for outbound TLS; yt-dlp for the watch-view transcript
          # ingest. /data is the TASK_DATA_ROOT volume.
          contents = with pkgs; [
            task-server
            git
            curl
            cacert
            bashInteractive
            coreutils
            yt-dlp
          ];
          extraCommands = ''
            mkdir -p data tmp
          '';
          config = {
            Entrypoint = [ "/bin/task-server" ];
            Env = [
              "TASK_BUILD_REV=${config.fts.buildRev}"
              "TASK_DATA_ROOT=/data"
              "TASK_SERVER_BIND=0.0.0.0:8080"
              "RUST_LOG=info"
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "GIT_SSL_CAINFO=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "PATH=/bin"
            ];
            ExposedPorts = { "8080/tcp" = { }; };
            Volumes = { "/data" = { }; };
            WorkingDir = "/data";
          };
        };
      };
    };
}
