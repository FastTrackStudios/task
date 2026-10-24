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

      # Two git-dep vendor fixes. Without them no pure package builds
      # (task-desktop, task-server); the deploy and dev builds never saw
      # it because they compile in the dev shell, where cargo checks out
      # git deps itself. (The FTS monorepo needs two others, for
      # reaper-low and libspa-sys; neither dep exists in this tree.)
      #
      # 1. FastTrackStudios/vendor v0.4.0 (b5844160) commits a stray
      #    `phon/Cargo.toml.orig`, a file name `cargo package` reserves.
      #    crane lists each crate's files with `cargo package -l`, which
      #    then fails ("invalid inclusion of reserved file name
      #    Cargo.toml.orig"). It is never a real source file in a git dep,
      #    so it is deleted from every checkout.
      #
      # 2. FastTrackStudios/editor's `editor-view` does
      #    `include_str!("../../../crates/editor/assets/editor.css")`, a
      #    file of a sibling crate in the same repo. Cargo checks out the
      #    whole repo for a git dep, so that resolves; crane copies each
      #    crate out on its own, so it does not. For that repo, keep the
      #    whole checkout in the output (`.tree`) and make each crate's
      #    entry a symlink into it: `..` then walks the real repo layout.
      #    The per-crate steps are crane's own (empty checksum, workspace
      #    inheritance resolved in place).
      wholeTreeRepos = [ "editor-view" ];
      keepWholeTree = ps: lib.any (p: builtins.elem p.name wholeTreeRepos) ps;

      cargoVendorDir = craneLib.vendorCargoDeps {
        src = taskSrc;
        overrideVendorGitCheckout = ps: drv: drv.overrideAttrs (old: {
          postPatch = (old.postPatch or "") + ''
            find . -name Cargo.toml.orig -type f -delete
          '';
        } // lib.optionalAttrs (keepWholeTree ps) {
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r . $out/.tree
            chmod -R u+w $out/.tree
            declare -A existing_crates
            while read -r cargoToml; do
              crate=$(
                cargo metadata --format-version 1 --no-deps --manifest-path "$cargoToml" |
                jq -r '.packages[] | select(.manifest_path == "'"$cargoToml"'") | "\(.name)-\(.version)"'
              )
              [ -n "$crate" ] || continue
              if [[ -n "''${existing_crates[$crate]:-}" ]]; then
                >&2 echo "warning: skipping duplicate package $crate found at $cargoToml"
                continue
              fi
              dir="$(dirname "$cargoToml")"
              echo '{"files":{}, "package":null}' > "$dir/.cargo-checksum.json"
              crane-resolve-workspace-inheritance "$cargoToml" > "$cargoToml.resolved"
              ln -s "$dir" "$out/$crate"
              existing_crates[$crate]=1
            done < <(find "$out/.tree" -name Cargo.toml | sort)
            # Every manifest is resolved before any is replaced: resolving
            # a member reads the workspace root's [workspace.package].
            find "$out/.tree" -name Cargo.toml.resolved | while read -r r; do
              mv "$r" "''${r%.resolved}"
            done
            runHook postInstall
          '';
        });
      };
    in
    {
      fts.craneLib = craneLib;
      fts.src = taskSrc;

      fts.commonArgs = {
        src = taskSrc;
        inherit cargoVendorDir;
        strictDeps = true;
        nativeBuildInputs = with pkgs; [ pkg-config ];
        buildInputs = with pkgs; [ openssl ];
      };

      # Git rev baked into the deployable images so a running deployment
      # can say WHICH commit it serves (version.json / TASK_BUILD_REV).
      # Only the cheap wrapper layers depend on it — the expensive
      # cargo/wasm derivations stay rev-free and cached.
      #
      # A release build stamps the tag's version into its checkout
      # (scripts/release.sh stamp), so the tree is dirty and self.rev is
      # gone; those jobs pass the tagged commit as FTS_BUILD_REV with
      # `--impure`. Pure evaluation reads getEnv as "", so every other build
      # is unchanged.
      fts.buildRev =
        let
          fromEnv = builtins.getEnv "FTS_BUILD_REV";
        in
        if fromEnv != "" then fromEnv else self.rev or self.dirtyRev or "unknown";
    };
}
