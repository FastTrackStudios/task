# crane plumbing shared by every deployable package (fills fts.craneLib,
# fts.src, fts.commonArgs, fts.buildRev).
{ inputs, self, ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      # The whole repo is the build source: ONE workspace, so the root
      # Cargo.lock drives vendoring and the [patch.crates-io] entries
      # resolve the same way they do outside nix. Keep the filter
      # minimal — the flake source is already the tracked git tree — and
      # only strip build output that would churn the store copy.
      taskSrc = lib.cleanSourceWith {
        src = ../..;
        filter = path: type:
          let name = builtins.baseNameOf (toString path); in
          !(builtins.elem name [ "target" "node_modules" ".git" "result" "dist" ]);
      };

      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain config.fts.rustToolchain;
    in
    {
      fts.craneLib = craneLib;
      fts.src = taskSrc;

      # No vendor overrides here (the FTS monorepo needs two: a WDL
      # submodule injection for reaper-low, and a bindgen scratch-dir fix
      # for libspa-sys/pipewire-sys). Neither dep exists in this tree, so
      # crane's default vendoring is enough.
      fts.commonArgs = {
        src = taskSrc;
        strictDeps = true;
        nativeBuildInputs = with pkgs; [ pkg-config ];
        buildInputs = with pkgs; [ openssl ];
      };

      # Git rev baked into the deployable images so a running deployment
      # can say WHICH commit it serves (version.json / TASK_BUILD_REV).
      # Only the cheap wrapper layers depend on it — the expensive
      # cargo/wasm derivations stay rev-free and cached.
      fts.buildRev = self.rev or self.dirtyRev or "unknown";
    };
}
