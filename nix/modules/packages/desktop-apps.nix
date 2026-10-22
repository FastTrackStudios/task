# The Dioxus desktop GUI — `nix run .#task-desktop`. Not the
# bundled/notarized .app release artifact (that's `dx bundle` via
# deploy/); same crane shape as task-server.nix (cargoArtifacts = null —
# the workspace is too large/build.rs-heavy for crane's dummy-src deps
# split).
#
# On Linux this is the RELEASE artifact NixOS hosts install: every
# release pushes it to fasttrackstudio.cachix.org (desktop-cache.yml), and
# nix-fleet installs `github:FastTrackStudios/task/<tag rev>#task-desktop`
# into a profile. So on Linux it is built the way the app ships, and it
# has to launch OUTSIDE the dev shell:
#
#   - Built with `dx build --release --linux`, not plain cargo. A release
#     binary is a "bundled app" to manganis, so every `asset!()` (the
#     Tailwind sheet, fonts, editor CSS, focus-trap.js) resolves to the
#     bundled path dx writes into the binary at link time. Plain cargo
#     leaves the placeholder ("This should be replaced by dx as part of
#     the build process...") and the window renders as unstyled HTML. dx
#     puts the processed assets in `assets/` beside the binary, which is
#     where dioxus-asset-resolver looks; both live in
#     lib/task-app-desktop/, with bin/ holding a wrapper.
#   - wrapGAppsHook3: GSettings schemas (GTK aborts on a file chooser with
#     "No GSettings schemas are installed"), GIO modules (glib-networking:
#     TLS for anything WebKit fetches itself), gdk-pixbuf loaders.
#   - LD_LIBRARY_PATH for what is dlopen'd rather than linked, which the
#     fixup's RPATH shrink therefore drops: the tray
#     (libayatana-appindicator), GL/Vulkan, wayland and xkbcommon.
#   - WEBKIT_DISABLE_COMPOSITING_MODE=1 by default, as in the dev shell:
#     WebKitGTK's accelerated compositing gives a white window on NixOS.
#
# `task` (the CLI) and `fts-files-daemon` (the sync agent, which the app
# looks for NEXT TO its own executable and registers by its own path as
# a systemd user unit) are plain cargo bins installed beside the window
# binary, unwrapped, and linked into bin/.
#
# On Darwin it stays a plain cargo build for local use.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      inherit (pkgs.stdenv) isLinux;

      # Reuse the toolchain's own GUI dep list (toolchain.nix already
      # assembled webkitgtk/gtk3/x11/vulkan for Linux, libiconv for
      # Darwin) instead of duplicating it — commonArgs.buildInputs alone
      # only carries openssl, which is enough for headless crates
      # (task-server) but not a wry/tao desktop window.
      guiArgs = {
        buildInputs = config.fts.buildInputs
          ++ lib.optionals isLinux (with pkgs; [ gsettings-desktop-schemas glib-networking ]);
        # python3 explicitly: stylo's build.rs (Blitz/dioxus-native, a
        # transitive dep of the desktop GUI) shells out to `python3` and
        # needs it on PATH, not just linkable.
        nativeBuildInputs = config.fts.nativeBuildInputs ++ [ pkgs.python3 ]
          ++ lib.optionals isLinux ([ config.fts.dx.cli ]
            ++ (with pkgs; [ wrapGAppsHook3 copyDesktopItems ]));
      };

      # dlopen'd at runtime (see the header), so invisible to the linker.
      runtimeLibs = lib.makeLibraryPath (with pkgs; [
        libayatana-appindicator
        libGL
        vulkan-loader
        wayland
        libxkbcommon
      ]);

      desktopItem = pkgs.makeDesktopItem {
        name = "task";
        desktopName = "Task";
        comment = "FastTrackStudio Task";
        exec = "task-app-desktop";
        icon = "task";
        categories = [ "Office" "ProjectManagement" ];
        startupWMClass = "task-app-desktop";
      };

      # assets/tailwind.css is generated, not committed; build it from the
      # one shared input up front, as web-bundles.nix does (dx would also
      # run Tailwind itself, but reads assets/ before cargo runs).
      tailwind = ''
        bash scripts/architect-ui-link.sh
        (cd apps && tailwindcss -i tailwind.css -o desktop/assets/tailwind.css)
      '';

      task-desktop = config.fts.craneLib.buildPackage (config.fts.commonArgs // guiArgs // {
        pname = "task-app-desktop";
        # The release's version, from `[workspace.package]`.
        version = (lib.importTOML ../../../Cargo.toml).workspace.package.version;
        cargoArtifacts = null;
        cargoExtraArgs = "--package task-app-desktop";
        doCheck = false;
        preBuild = tailwind;
        meta.mainProgram = "task-app-desktop";
      } // lib.optionalAttrs isLinux {
        # Hermetic dx: no network in the sandbox.
        NO_DOWNLOADS = "1";
        doNotPostBuildInstallCargoBinaries = true;
        preBuild = "";
        buildPhaseCargoCommand = ''
          export HOME="$TMPDIR/dx-home"
          mkdir -p "$HOME"
          ${tailwind}
          (cd apps/desktop && dx build --release --linux)
          cargo build --release --package task-app-desktop \
            --bin task --bin fts-files-daemon
        '';
        installPhaseCommand = ''
          app="$(dirname "$(find target/dx/task-app-desktop/release -type f -name task-app-desktop -perm -u+x | head -1)")"
          [ -d "$app/assets" ] || { echo "dx wrote no assets/ beside the binary at '$app'" >&2; exit 1; }
          dir=$out/lib/task-app-desktop
          mkdir -p $dir $out/bin
          cp -R "$app/." $dir/
          for b in task fts-files-daemon; do
            install -Dm755 target/release/$b $dir/$b
            ln -s $dir/$b $out/bin/$b
          done
          # The PWA's icon is the app's icon (nix-fleet's own entry used it).
          install -Dm644 ${../../../apps/web/assets/icon-512.png} \
            $out/share/icons/hicolor/512x512/apps/task.png
        '';
        desktopItems = [ desktopItem ];
        # One wrapper, for the window only (the hook would wrap every bin/).
        dontWrapGApps = true;
        postFixup = ''
          makeWrapper $out/lib/task-app-desktop/task-app-desktop $out/bin/task-app-desktop \
            "''${gappsWrapperArgs[@]}" \
            --prefix LD_LIBRARY_PATH : ${runtimeLibs} \
            --set-default WEBKIT_DISABLE_COMPOSITING_MODE 1
        '';
      });
    in
    {
      packages = { inherit task-desktop; };
    };
}
