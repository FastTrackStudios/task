# CI shell — the default shell minus every interactive convenience.
# No .env sourcing, no dx/Playwright/editor tooling, no sccache. Toolchain
# + native headers + the env the build scripts need, nothing else.
# Workflows enter it via `nix develop .#ci`.
#
# tailwindcss and node ARE here, despite "CI drives plain cargo":
# apps/{web,desktop,mobile}/assets/tailwind.css is gitignored build output
# that `asset!()` demands at compile time. `dx` generates it during a
# normal dx build, but CI drives plain cargo, so the workflow runs the
# `css` recipe first — which needs the binary AND the tailwindcss module
# resolvable from node_modules.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }: {
    devShells.ci = pkgs.mkShell ({
      packages = [ config.fts.rustToolchain pkgs.cargo-nextest ]
      # cargo-rail — the `plan` job's change selection (and `cargo rail
      # unify --check` as a hygiene gate). Store-sourced only.
      ++ lib.optionals (config.fts.cargoRail != null) [ config.fts.cargoRail ]
      ++ config.fts.buildInputs
      # The shared native tool list (pkg-config, bindgen, tailwindcss),
      # consumed from toolchain.nix instead of hand-repeating entries
      # here, so the two shells can't drift.
      ++ config.fts.nativeBuildInputs
      ++ [
        # `just` so the workflow invokes the `css` recipe instead of
        # repeating the tailwindcss commands, and node for the module
        # resolution that recipe's `@import "tailwindcss"` needs.
        pkgs.just
        pkgs.nodejs_22
        # git — task-server tests and the snapshot engine shell out to it.
        pkgs.git
        # mold — CI selects it for the host target via
        # CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS in
        # .github/workflows/checks.yml. A `nextest run --workspace` links
        # 326 test binaries and GNU ld does that serially; the flag is set
        # in the workflow rather than .cargo/config.toml so a developer
        # shell without mold still links.
        pkgs.mold
      ];

      # Seeded cargo-home bins resolve from PATH — never installed from
      # here. APPEND, don't prepend: GitHub-hosted runners ship a rustup
      # stable in ~/.cargo/bin that would otherwise shadow the pinned nix
      # rustc (first symptom: "can't find crate for core" on wasm32).
      shellHook = ''
        export PATH="$PATH:$HOME/.cargo/bin"
      '';
    }
    // config.fts.shellEnv);
  };
}
