# ui-lab (pnpm + Vite) + its static-site image.
# Its own pnpm workspace under apps/ui-lab (vendor/* holds the vendored
# @bearcove/vox-* TS runtime as workspace:* deps). Fetcher, config hook,
# and build pnpm MUST be the same major — pin all three to pnpm_9.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      ui-lab = pkgs.stdenv.mkDerivation (finalAttrs: {
        pname = "task-ui-lab";
        version = "0.0.0";
        src = ../../../apps/ui-lab;
        nativeBuildInputs = [
          pkgs.nodejs_22
          pkgs.pnpm_9
          pkgs.pnpm_9.configHook
        ];
        pnpmDeps = pkgs.pnpm_9.fetchDeps {
          inherit (finalAttrs) pname version src;
          fetcherVersion = 2;
          hash = "sha256-JBWJhg81dixFwSc8GZg0yJcSyd38pR08VLcH81KkId4=";
        };
        buildPhase = ''
          runHook preBuild
          pnpm build
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out/www
          cp -R dist/* $out/www/
          runHook postInstall
        '';
      });
    in
    {
      packages = { inherit ui-lab; }
      // lib.optionalAttrs pkgs.stdenv.isLinux {
        # CI's contract name (the image itself is `task-ui-lab`).
        task-ui-lab-image = config.fts.mkStaticSite {
          name = "task-ui-lab";
          siteRoot = "${ui-lab}/www";
        };
      };
    };
}
