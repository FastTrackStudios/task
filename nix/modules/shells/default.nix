# Dev shell — one shell for the whole workspace. Toolchain + native/wasm
# env so `cargo` / `dx` behave identically for the server, the web app,
# the desktop window and the mobile builds.
#
# dx comes from the store (fts.dx — the nixpkgs-dx pin, the same trio the
# hermetic web bundle uses), never `cargo install dioxus-cli`.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }: {
    devShells.default = pkgs.mkShell ({
      packages = (with pkgs; [
        # The command runner the repo drives through (./Justfile).
        just
        cargo-watch
        cargo-nextest
        bacon
        # Node + pnpm: the Playwright suites under apps/tests and
        # features/editor/tests; also the tailwindcss module resolution that
        # `@import "tailwindcss"` needs.
        nodejs_22
        pnpm
        # git — task-server's snapshot engine shells out to it.
        git
      ])
      ++ [
        config.fts.rustToolchain
        config.fts.dx.cli          # dx — prebuilt, no cargo install
        config.fts.dx.wasmBindgen  # wasm-bindgen-cli 0.2.127 (lock match)
        config.fts.dx.binaryen     # wasm-opt 129 — what dx pins
      ]
      # cargo-rail — graph-aware workspace maintenance and the CI change
      # selection this repo's checks.yml plans with. Pinned + patched
      # build, NOT nixpkgs' stale 0.7.0 (nix/modules/cargo-rail.nix).
      ++ lib.optionals (config.fts.cargoRail != null) [ config.fts.cargoRail ]
      ++ lib.optionals pkgs.stdenv.isLinux [
        # linuxdeploy — dx's AppImage bundler (`dx bundle --package-types
        # appimage`) shells out to it. Not in the flake's (older) nixpkgs
        # pin, so take it from the current-unstable nixpkgs-dx set.
        config.fts.pkgsDx.linuxdeploy
      ]
      ++ config.fts.buildInputs
      ++ config.fts.nativeBuildInputs;

      # Playwright browsers for apps/tests/playwright. pkgsDx's
      # playwright-driver matches the @playwright/test pin — keep them in
      # lockstep. Without this Playwright falls back to its own
      # downloaded chromium, which can't load shared libs on NixOS
      # (libnspr4.so).
      PLAYWRIGHT_BROWSERS_PATH = config.fts.pkgsDx.playwright-driver.browsers;
      PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";

      shellHook = ''
        [ -f .env ] && { set -a; source .env; set +a; }

        # No RUSTC_WRAPPER. sccache lived here once (a compiler cache in
        # front of rustc — full-wipe recovery, nothing more: 0% hits
        # across worktrees since the target path keys the cache), but it
        # cannot coexist with `dx serve`, which interposes its own rustc
        # wrapper — sccache is handed dx's wrapper as "the compiler",
        # probes it as C, and every dx build dies with "Compiler not
        # supported". A cache that breaks the app dev loop costs more
        # than the wipes it saves.
        unset RUSTC_WRAPPER
        # Append (not prepend): store-provided tools (dx, wasm-bindgen)
        # must win over stale cargo-installed copies; cargo bins only
        # need to be reachable.
        export PATH="$PATH:$HOME/.cargo/bin:$HOME/.local/bin"

        echo ""
        echo "  Task dev shell"
        echo "  ─────────────────────────────────────────────"
        echo "  just css   # generated tailwind sheets"
        echo "  cargo check --workspace"
        echo "  cargo run -p task-server        # the server"
        echo "  (cd apps/web && dx serve --web --hot-patch false)"
        echo ""
        echo "  Rust: $(rustc --version)"
        echo "  dx:   $(dx --version 2>/dev/null || echo 'not available')"
        echo ""
      '';
    }
    // config.fts.shellEnv);
  };
}
