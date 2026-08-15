# static-web-server image factory (fills fts.mkStaticSite): serve $root
# on :8080, SPA-fallback unknown paths to index.html. HTML is `no-cache`
# (nix-store mtimes are 1970 — heuristic freshness would pin index.html
# for months); /assets/** is immutable-forever (dx content-hashes asset
# URLs). Real copy, not symlinks: static-web-server denies files
# resolving outside its root.
#
# `dockerTools.streamLayeredImage` produces an *executable* that streams
# a docker-archive tarball to stdout:
#   $(nix build --print-out-paths .#task-web-image) \
#     | skopeo copy docker-archive:/dev/stdin docker://…
# Linux-only (consumers guard with lib.optionalAttrs).
{ ... }:
{
  perSystem = { pkgs, config, ... }: {
    fts.mkStaticSite = { name, tag ? "latest", siteRoot }:
      let
        versionedRoot = pkgs.runCommand "${name}-root" { } ''
          mkdir -p $out
          cp -a ${siteRoot}/. $out/
          chmod u+w $out
          echo '{"rev":"${config.fts.buildRev}"}' > $out/version.json
        '';
        swsConfig = pkgs.writeText "sws.toml" ''
          [general]
          host = "0.0.0.0"
          port = 8080
          root = "${versionedRoot}"
          page-fallback = "${versionedRoot}/index.html"
          log-level = "info"
          compression-static = true

          [advanced]
          [[advanced.headers]]
          source = "/**"
          [advanced.headers.headers]
          Cache-Control = "no-cache"

          [[advanced.headers]]
          source = "/assets/**"
          [advanced.headers.headers]
          Cache-Control = "public, max-age=31536000, immutable"
        '';
      in
      pkgs.dockerTools.streamLayeredImage {
        inherit name tag;
        contents = [ pkgs.static-web-server pkgs.cacert ];
        config = {
          Entrypoint = [ "/bin/static-web-server" ];
          Cmd = [ "--config-file" "${swsConfig}" ];
          ExposedPorts = { "8080/tcp" = { }; };
          Env = [ "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" ];
        };
      };
  };
}
