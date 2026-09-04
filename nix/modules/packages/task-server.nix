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
      # The image around any package that provides bin/task-server — the
      # pure build here, or the runner-built binary of the deploy's fast
      # path (packages/prebuilt-images.nix). One definition, so the two
      # images cannot drift in contents or config.
      mkServerImage = { server }:
        pkgs.dockerTools.streamLayeredImage {
          name = "task-server";
          tag = "latest";
          # git + curl: the snapshot engine shells out to them, and so
          # do repo-sourced wikis (`wiki_live::repo_source` clones,
          # fetches and pushes with the `git` binary — `wiki.source.*`);
          # cacert for outbound TLS, which `GIT_SSL_CAINFO` below hands
          # to git for https clones; yt-dlp for the watch-view
          # transcript ingest. /data is the TASK_DATA_ROOT volume.
          contents = with pkgs; [
            server
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
    in
    {
      fts.mkServerImage = mkServerImage;
      packages = { inherit task-server; }
      // lib.optionalAttrs pkgs.stdenv.isLinux {
        task-server-image = mkServerImage { server = task-server; };
      };
    };
}
