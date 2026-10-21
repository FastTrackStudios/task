# The Dioxus desktop GUI, built as an ordinary crane package —
# `nix run .#task-desktop`. Not the bundled/notarized .app release
# artifact (that's `dx bundle` via deploy/); this is a plain native
# binary, same shape as task-server.nix (cargoArtifacts = null — the
# workspace is too large/build.rs-heavy for crane's dummy-src deps split).
#
# `desktop`/`launch` are ordinary cargo features on non-wasm/non-ios
# targets (see apps/desktop/Cargo.toml), so no `dx` involvement is needed
# to produce a runnable binary — dx is only for the wasm/mobile/.app
# pipelines.
#
# On Linux this is also the RELEASE artifact NixOS hosts install: every
# release pushes it to fasttrackstudio.cachix.org (desktop-cache.yml), and
# nix-fleet installs `github:FastTrackStudios/task/<tag rev>#task-desktop`
# into a profile. So it has to launch OUTSIDE the dev shell, where nothing
# provides the runtime env toolchain.nix's fts.shellEnv sets:
#
#   - wrapGAppsHook3: GSettings schemas (GTK aborts on a file chooser with
#     "No GSettings schemas are installed"), GIO modules (glib-networking:
#     TLS for anything WebKit fetches itself), gdk-pixbuf loaders.
#   - LD_LIBRARY_PATH for what is dlopen'd rather than linked, which the
#     fixup's RPATH shrink therefore drops: the tray
#     (libayatana-appindicator), GL/Vulkan, wayland and xkbcommon.
#   - WEBKIT_DISABLE_COMPOSITING_MODE=1 by default, as in the dev shell:
#     WebKitGTK's accelerated compositing gives a white window on NixOS.
#
# Only `task-app-desktop` is wrapped. `task` (the CLI) and
# `fts-files-daemon` (the sync agent, which registers its own path as a
# systemd user unit) stay the real binaries.
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
          ++ lib.optionals isLinux (with pkgs; [ wrapGAppsHook3 copyDesktopItems ]);
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

      task-desktop = config.fts.craneLib.buildPackage (config.fts.commonArgs // guiArgs // {
        pname = "task-app-desktop";
        # The release's version, from `[workspace.package]`.
        version = (lib.importTOML ../../../Cargo.toml).workspace.package.version;
        cargoArtifacts = null;
        cargoExtraArgs = "--package task-app-desktop";
        doCheck = false;
        meta.mainProgram = "task-app-desktop";
      } // lib.optionalAttrs isLinux {
        desktopItems = [ desktopItem ];
        # The PWA's icon is the app's icon (nix-fleet's own entry used it).
        postInstall = ''
          install -Dm644 ${../../../apps/web/assets/icon-512.png} \
            $out/share/icons/hicolor/512x512/apps/task.png
        '';
        # Wrap the window only; the hook would otherwise wrap every bin/.
        dontWrapGApps = true;
        postFixup = ''
          wrapGApp $out/bin/task-app-desktop \
            --prefix LD_LIBRARY_PATH : ${runtimeLibs} \
            --set-default WEBKIT_DISABLE_COMPOSITING_MODE 1
        '';
      });
    in
    {
      packages = { inherit task-desktop; };
    };
}
