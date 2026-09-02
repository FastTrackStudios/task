# The dx web bundle (task-webapp) + its static-site OCI image.
# dx bundle → $out/www (+ brotli pre-compression).
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      inherit (config.fts) craneLib commonArgs mkStaticSite;

      # Shared env for the dx web-bundle build: crates that compile C to
      # wasm via cc::Build (arborium's tree-sitter runtime + grammars,
      # ring) need an unwrapped clang targeting wasm32 and llvm-ar — the
      # cc-wrapper injects host-only hardening flags clang rejects for
      # wasm. Without these the C symbols stay as unresolved
      # `(import "env" ...)` entries and the shipped bundle white-screens.
      # Mirrors devShells.default exactly.
      dxWebEnv = {
        CC_wasm32_unknown_unknown = "${pkgs.llvmPackages_18.clang-unwrapped}/bin/clang";
        AR_wasm32_unknown_unknown = "${pkgs.llvmPackages_18.bintools-unwrapped}/bin/llvm-ar";
        CFLAGS_wasm32_unknown_unknown = "-isystem ${pkgs.llvmPackages_18.clang}/resource-root/include";
        # Hermetic dx: no network in the sandbox. With NO_DOWNLOADS set,
        # dx resolves wasm-opt / wasm-bindgen from PATH instead of
        # fetching them from GitHub.
        NO_DOWNLOADS = "1";
      };

      dxWebNativeInputs = commonArgs.nativeBuildInputs ++ [
        config.fts.dx.cli          # dx (nixpkgs-dx)
        config.fts.dx.wasmBindgen  # 0.2.127, matches the lock
        config.fts.dx.binaryen     # wasm-opt 129 — what dx pins
      ] ++ (with pkgs; [
        tailwindcss_4
        # Pre-compression for --compression-static serving.
        brotli
        llvmPackages_18.clang-unwrapped
        llvmPackages_18.bintools-unwrapped
      ]);

      # The dx build runs from the app dir but writes to the
      # WORKSPACE-ROOT target/dx/<name>/release/web/public.
      mkDxWebBundle = { pname, appDir, dxName, preBuild ? "" }:
        craneLib.buildPackage (commonArgs // dxWebEnv // {
          inherit pname;
          version = "0.1.0";
          cargoArtifacts = null;
          cargoExtraArgs = "--manifest-path ${appDir}/Cargo.toml";
          nativeBuildInputs = dxWebNativeInputs;
          doNotPostBuildInstallCargoBinaries = true;
          buildPhaseCargoCommand = ''
            export HOME="$TMPDIR/dx-home"
            mkdir -p "$HOME"
            ${preBuild}
            cd ${appDir}
            # The profile is `[profile.wasm-release]` in the root
            # Cargo.toml — dx's default name for a web release build, so
            # no --profile flag; that is where opt-level/LTO/panic live.
            #
            # --debug-symbols false: drop DWARF for a smaller release
            # bundle (and it sidesteps DWARF-version mismatches in
            # wasm-opt).
            # --wasm-split + --features wasm-split: cut the binary into
            # a main chunk plus one lazily fetched chunk per route and
            # per plugin app (`dioxus-router/wasm-split` and
            # `task_plugin_ui::lazy_view!`). The cargo feature compiles
            # the lazy loaders in; the dx flag runs the splitter that
            # writes the chunks they fetch. One without the other is a
            # broken bundle, so they travel together. Needs the dx from
            # nix/modules/dx.nix (the #5668 fork) — the published alpha
            # panics on this app. Details in docs/task-webapp.md.
            dx build --release --platform web --debug-symbols false \
              --wasm-split --features wasm-split
          '';
          # buildPhase ends inside ${appDir}; anchor the copy at the
          # workspace root explicitly.
          installPhaseCommand = ''
            mkdir -p $out/www
            srcdir="$(pwd)"
            case "$srcdir" in */${appDir}) srcdir="''${srcdir%/${appDir}}";; esac
            cp -R "$srcdir/target/dx/${dxName}/release/web/public/." $out/www/
            # Pre-compress text/wasm so static-web-server's
            # --compression-static serves .br variants (the multi-MB
            # wasm goes over the wire at brotli size).
            find $out/www -type f \( -name '*.wasm' -o -name '*.js' \
              -o -name '*.css' -o -name '*.html' -o -name '*.json' \
              -o -name '*.svg' \) -exec brotli --keep --quality=9 {} +
          '';
          doCheck = false;
        });

      task-webapp = mkDxWebBundle {
        pname = "task-webapp";
        appDir = "apps/web";
        dxName = "task-app-web";
        # assets/tailwind.css is generated, not committed. dx detects the
        # `tailwind.css` at the crate root and builds it, but it reads
        # assets/ before cargo runs, so build it up front to avoid the
        # race — from the ONE shared input at apps/tailwind.css.
        #
        # cwd is irrelevant to the result: the input sets `source(none)`
        # and names every source explicitly, so the sheet is identical
        # from anywhere. Run from apps/ purely for the short path.
        preBuild = ''
          (cd apps && tailwindcss -i tailwind.css -o web/assets/tailwind.css)
        '';
      };
    in
    {
      packages = { inherit task-webapp; }
      // lib.optionalAttrs pkgs.stdenv.isLinux {
        task-web-image = mkStaticSite {
          name = "task-web";
          siteRoot = "${task-webapp}/www";
        };
      };
    };
}
