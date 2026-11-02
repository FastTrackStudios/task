# Task's dev environment. devenv-fleet's modules (devenv.yaml) bring the
# fleet toolchain (rust 1.99.1, the rust-toolchain.toml pin), mold,
# cargo-hakari, cargo-rail, prek and just (`rust`); dx, wasm-bindgen,
# binaryen and tailwindcss (`dioxus`); the iOS cross env on macOS
# (`darwin`); and a local Teley behind an OTLP collector (`telemetry`).
# This file adds what this workspace links against and runs, and the
# processes `devenv up` starts.
#
# .env is NOT read here (devenv's dotenv integration copies the file, and
# its secrets, into the nix store): .envrc loads it into the shell with
# direnv's `dotenv_if_exists`, and the processes source it themselves.
{ pkgs, lib, config, inputs, ... }:
let
  pkgsDx = import inputs.nixpkgs-dx { inherit (pkgs.stdenv) system; };

  # The native libraries cargo builds link against. This list is shorter
  # than the FastTrackStudio monorepo's on purpose: it was derived from
  # this workspace's Cargo.lock and checked against the -sys build scripts
  # a `cargo check --workspace` actually runs. Absent: alsa/jack/pipewire
  # (no native cpal host), udev (no hidapi), onnxruntime (no ort), avahi.
  common = with pkgs; [
    # openssl-sys (reqwest's native-tls path).
    openssl
    openssl.dev
    # yeslogic-fontconfig-sys, and the Blitz/Vello text stack
    # (dioxus-native) dlopens them at run time.
    fontconfig
    freetype
    # aws-lc-sys (rustls' default provider) drives its C build through
    # cmake; stylo's build.rs shells out to python3 on the Blitz path.
    cmake
    python3
  ];
  linux = with pkgs; lib.optionals stdenv.isLinux [
    # The wry/tao webview stack behind apps/desktop and `dx serve
    # --platform desktop`: webkit2gtk, soup3 and gtk-sys are each a
    # pkg-config lookup at build time.
    glib
    gtk3
    gdk-pixbuf
    pango
    cairo
    atk
    harfbuzz
    libsoup_3
    webkitgtk_4_1
    xdotool
    # The system tray (tray-icon, libappindicator-sys). dlopen'd at run
    # time, so it is on LD_LIBRARY_PATH too.
    libayatana-appindicator
    # tao/winit windowing and wgpu/vello rendering.
    libx11
    libxcursor
    libxrandr
    libxi
    libxcb
    libxkbcommon
    wayland
    libGL
    vulkan-loader
  ];
in
{
  packages = common ++ linux ++ (with pkgs; [
    # bindgen (clang-sys): LIBCLANG_PATH and BINDGEN_EXTRA_CLANG_ARGS.
    rustPlatform.bindgenHook
    cargo-watch
    cargo-nextest
    bacon
    # The Playwright suites (apps/tests, features/editor/tests), and the
    # module resolution `@import "tailwindcss"` needs.
    nodejs_22
    pnpm
    # task-server's snapshot engine shells out to git.
    git
    # prek's hooks for shell scripts and workflows (prek.toml).
    shellcheck
    actionlint
  ])
  ++ lib.optionals pkgs.stdenv.isLinux [
    # The server's rendition pipeline probes for ffmpeg at boot (media
    # transcoding is off without it), and `admin demo` synthesises the
    # example video deliverables with it. A run-time tool, not linked.
    pkgs.ffmpeg
    # Reclaims stale target/ artifacts; cargo never collects them.
    pkgs.cargo-sweep
    # dx's AppImage bundler (`dx bundle --package-types appimage`).
    pkgsDx.linuxdeploy
  ];

  env = {
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    OPENSSL_DIR = "${pkgs.openssl.dev}";
    OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
    # Playwright's browsers from the nixpkgs-dx set, whose playwright-driver
    # matches the @playwright/test pin; its own download can't load shared
    # libraries on NixOS (libnspr4.so).
    PLAYWRIGHT_BROWSERS_PATH = "${pkgsDx.playwright-driver.browsers}";
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
  }
  // lib.optionalAttrs pkgs.stdenv.isLinux {
    # Point the loader at what the linker found, so desktop test binaries
    # run from this shell. libstdc++ too: on a hosted (Ubuntu) runner a
    # nix-built test binary fails without it.
    LD_LIBRARY_PATH = lib.makeLibraryPath (common ++ linux ++ [ pkgs.stdenv.cc.cc.lib ]);
    XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}";
    # WebKitGTK's accelerated compositing fails on NixOS (GBM buffer error,
    # white window): https://github.com/NixOS/nixpkgs/issues/32580
    WEBKIT_DISABLE_COMPOSITING_MODE = "1";
  };

  fleet.banner = ''
    just css                  generated tailwind sheets
    devenv up                 server :18080 + web + Teley
    devenv up server          the server alone
  '';

  # `devenv up`: the server, the web app proxying to it (apps/web/Dioxus.toml
  # forwards /org, /media and /blobs to :18080), and the telemetry module's
  # Teley and collector. `devenv up -d` then `devenv processes wait` from a
  # script or an agent.
  processes.server = {
    # .env's TASK_SERVER_BIND, if set, wins; .env.example's is this one.
    exec = ''
      set -a; [ -f .env ] && . ./.env; set +a
      exec cargo run --release -p task-server
    '';
    env = { TASK_SERVER_BIND = "127.0.0.1:18080"; } // config.fleet.telemetry.processEnv;
    ready.exec = "${pkgs.curl}/bin/curl -sf -o /dev/null http://127.0.0.1:18080/health";
  };

  # 0.0.0.0, so the starcommand nginx reverse proxy reaches it over the LAN.
  # Without hot-patching: dx serve's default hot-patch path breaks wasm
  # builds (LinkError, a subsecond panic on edit).
  processes.web = {
    exec = ''
      set -a; [ -f .env ] && . ./.env; set +a
      just css
      cd apps/web && exec dx serve --web --addr 0.0.0.0 --hot-patch false
    '';
    after = [ "devenv:processes:server" ];
  };
}
