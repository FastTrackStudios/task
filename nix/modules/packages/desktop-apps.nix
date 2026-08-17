# The Dioxus desktop GUI, built as an ordinary crane package —
# `nix run .#task-desktop`. Not the bundled/notarized .app release
# artifact (that's `dx bundle` via deploy/); this is a plain native
# binary for local dev use, same shape as task-server.nix
# (cargoArtifacts = null — the workspace is too large/build.rs-heavy for
# crane's dummy-src deps split).
#
# `desktop`/`launch` are ordinary cargo features on non-wasm/non-ios
# targets (see apps/desktop/Cargo.toml), so no `dx` involvement is needed
# to produce a runnable binary — dx is only for the wasm/mobile/.app
# pipelines.
{ ... }:
{
  perSystem = { pkgs, config, ... }:
    let
      # Reuse the toolchain's own GUI dep list (toolchain.nix already
      # assembled webkitgtk/gtk3/x11/vulkan for Linux, libiconv for
      # Darwin) instead of duplicating it — commonArgs.buildInputs alone
      # only carries openssl, which is enough for headless crates
      # (task-server) but not a wry/tao desktop window.
      guiArgs = {
        buildInputs = config.fts.buildInputs;
        # python3 explicitly: stylo's build.rs (Blitz/dioxus-native, a
        # transitive dep of the desktop GUI) shells out to `python3` and
        # needs it on PATH, not just linkable.
        nativeBuildInputs = config.fts.nativeBuildInputs ++ [ pkgs.python3 ];
      };

      task-desktop = config.fts.craneLib.buildPackage (config.fts.commonArgs // guiArgs // {
        pname = "task-app-desktop";
        version = "0.1.0";
        cargoArtifacts = null;
        cargoExtraArgs = "--package task-app-desktop";
        doCheck = false;
        meta.mainProgram = "task-app-desktop";
      });
    in
    {
      packages = { inherit task-desktop; };
    };
}
