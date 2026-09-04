# The deploy's fast path: images assembled from artifacts the RUNNER
# already built, instead of a sandboxed rebuild of the whole workspace.
#
# The pure `task-server-image` / `task-web-image` compile every crate
# from scratch inside Nix (cargoArtifacts = null — no deps split), so a
# deploy paid 20–30 minutes of release build per commit however warm the
# runner's cargo cache was: the sandbox cannot see it. Here the workflow
# builds `task-server` and the dx web bundle in the dev shell against a
# persistent CARGO_TARGET_DIR on the runner's big disk — incremental
# across deploys, so a one-crate change is minutes — and hands the results
# in by path. Nix then does only what it is good at: the layered image,
# with the exact same contents and config as the pure variant (shared via
# `fts.mkServerImage` / `fts.mkStaticSite`).
#
# Impure on purpose: the paths arrive through the environment, so these
# attributes need `nix build --impure`. The runtime closure is still
# complete — the binary was linked against the dev shell's store paths,
# and Nix scans the copied file for store references, so glibc, openssl
# and friends ride into the image as before.
#
#   TASK_PREBUILT_SERVER_BIN=$CARGO_TARGET_DIR/release/task-server \
#     nix build --impure .#task-server-image-prebuilt
#   TASK_PREBUILT_WEB_ROOT=target/dx/task-app-web/release/web/public \
#     nix build --impure .#task-web-image-prebuilt
#
# The pure attributes stay for anyone without a warm runner.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      fromEnv = name:
        let v = builtins.getEnv name;
        in if v == "" then null else v;

      serverBin = fromEnv "TASK_PREBUILT_SERVER_BIN";
      webRoot = fromEnv "TASK_PREBUILT_WEB_ROOT";

      # A store copy of the runner-built binary, at the path the image's
      # Entrypoint expects.
      prebuiltServer = pkgs.runCommand "task-server-prebuilt" { } ''
        install -Dm755 ${/. + serverBin} $out/bin/task-server
      '';

      # The dx bundle as a store path, pre-compressed like the pure build
      # so static-web-server's --compression-static has its .br files.
      prebuiltWeb = pkgs.runCommand "task-webapp-prebuilt" {
        nativeBuildInputs = [ pkgs.brotli ];
      } ''
        mkdir -p $out/www
        cp -R ${builtins.path { path = webRoot; name = "task-webapp-public"; }}/. $out/www/
        chmod -R u+w $out/www
        find $out/www -type f \( -name '*.wasm' -o -name '*.js' \
          -o -name '*.css' -o -name '*.html' -o -name '*.json' \
          -o -name '*.svg' \) -exec brotli --keep --quality=9 {} +
      '';
    in
    {
      packages = lib.optionalAttrs pkgs.stdenv.isLinux (
        lib.optionalAttrs (serverBin != null) {
          task-server-image-prebuilt = config.fts.mkServerImage { server = prebuiltServer; };
        }
        // lib.optionalAttrs (webRoot != null) {
          task-web-image-prebuilt = config.fts.mkStaticSite {
            name = "task-web";
            siteRoot = "${prebuiltWeb}/www";
          };
        }
      );
    };
}
