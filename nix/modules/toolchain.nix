# The toolchain + native build environment (fills fts.rustToolchain,
# fts.buildInputs, fts.nativeBuildInputs, fts.shellEnv).
#
# This list is deliberately SHORTER than the FastTrackStudio monorepo's:
# it was derived from this workspace's Cargo.lock and then checked against
# what a `cargo check --workspace` actually compiles (the -sys build
# scripts that appear in target/debug/build), not copied. Notably absent,
# and why:
#
#   alsa / jack / pipewire — no `cpal`, `alsa-sys`, `jack-sys`,
#     `libspa-sys` or `pipewire-sys` in the lock. crates/player-ui does
#     depend on daw-standalone + session, but only with the
#     `decode`/`web`/`bootstrap` features: decoding (symphonia) and the
#     browser AudioWorklet path, never a native audio device backend.
#     Audio HEADERS would only come back with a native cpal host.
#   udev — no `hidapi`/`udev` crate (the raw-USB control-surface work
#     stayed in the FTS repo).
#   onnxruntime — no `ort` (Chatterbox TTS lives in the FTS session
#     guide, not here).
#   avahi — vox-discover is not in this tree.
#   mold — this repo's `.cargo/config.toml` does not select an
#     alternative linker, so nothing on PATH has to provide one.
{ ... }:
{
  perSystem = { pkgs, lib, config, ... }:
    let
      libPath = lib.makeLibraryPath (with pkgs;
        # stdenv.cc.cc.lib — libstdc++ for dynamically-linked C++ deps.
        # On NixOS hosts the system profile papers over its absence; on
        # GitHub-hosted (Ubuntu) runners nix-built test binaries fail to
        # load with "libstdc++.so.6: cannot open shared object file".
        [ stdenv.cc.cc.lib fontconfig freetype openssl ]
        ++ lib.optionals pkgs.stdenv.isLinux [
          libGL vulkan-loader gtk3 glib
          gdk-pixbuf pango cairo atk
          libx11 libxcb libxkbcommon wayland
          webkitgtk_4_1 libsoup_3 xdotool
        ]
      );
    in
    {
      # Rust toolchain — the FTS-wide pin (rust-toolchain.toml says the
      # same: 1.94.0 + wasm32), with the iOS/Intel targets on darwin for
      # apps/mobile and the macOS desktop builds.
      fts.rustToolchain = pkgs.rust-bin.stable."1.94.0".default.override {
        extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
        targets = [ "wasm32-unknown-unknown" ]
          ++ lib.optionals pkgs.stdenv.isDarwin [
            "aarch64-apple-ios"
            "aarch64-apple-ios-sim"
            "x86_64-apple-darwin"
          ];
      };

      fts.buildInputs = (with pkgs; [
        # openssl — openssl-sys (reqwest/native-tls path).
        openssl openssl.dev
        libiconv pkg-config
        # fontconfig/freetype — yeslogic-fontconfig-sys is in the lock and
        # the Blitz/Vello text stack (dioxus-native) dlopens them at
        # RUNTIME even when the default feature set doesn't compile their
        # -sys build scripts; they are also on LD_LIBRARY_PATH below.
        fontconfig freetype
        # cmake — aws-lc-sys (rustls' default provider) does compile in
        # this workspace (verified in the check log) and drives its C
        # build through cmake. python3 — stylo's build.rs shells out to
        # it on the dioxus-native/Blitz feature path.
        cmake python3
      ])
      ++ lib.optionals pkgs.stdenv.isLinux (with pkgs; [
        # The wry/tao webview stack behind `dx serve --platform desktop`
        # and apps/desktop: webkit2gtk + soup3 + gtk-sys are all in the
        # lock, and each is a pkg-config lookup at build time.
        glib gtk3 gdk-pixbuf pango cairo atk harfbuzz
        libsoup_3 webkitgtk_4_1 xdotool
        # tao/winit windowing + wgpu/vello rendering (ash, khronos-egl,
        # wayland-sys, xkbcommon-dl in the lock).
        libx11 libxcursor libxrandr libxi libxcb
        libxkbcommon wayland libGL vulkan-loader
      ])
      ++ lib.optionals pkgs.stdenv.isDarwin (with pkgs; [
        # No explicit apple-sdk here: the darwin stdenv already carries one,
        # and a second SDK in scope makes the cc wrapper abort with
        # "Multiple conflicting values defined for DEVELOPER_DIR".
        libiconv
      ]);

      fts.nativeBuildInputs = with pkgs; [
        pkg-config
        # bindgen (clang-sys) — libsqlite3-sys, aws-lc-sys, webkit2gtk.
        rustPlatform.bindgenHook
        # tailwindcss — apps/{web,desktop,mobile}/assets/tailwind.css is
        # generated build output that `asset!()` resolves at compile
        # time, so a plain `cargo check` needs it built first
        # (`just --justfile apps/Justfile --working-directory apps css`).
        tailwindcss_4
      ]
      ++ lib.optionals pkgs.stdenv.isLinux [
        # cargo-sweep — reclaims stale target/ artifacts; cargo never GCs.
        cargo-sweep
        # sccache — compiler cache, wired as RUSTC_WRAPPER in
        # nix/modules/shells/default.nix.
        sccache
      ];

      # Env every dev/CI shell needs — build-script and bindgen paths,
      # the wasm cross toolchain, runtime library paths.
      fts.shellEnv = {
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        OPENSSL_DIR = "${pkgs.openssl.dev}";
        OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
        # Unwrapped clang: the nix cc-wrapper injects hardening flags
        # (-fzero-call-used-regs) unsupported on wasm32 and leaks glibc
        # includes past -nostdlibinc (breaks ring). Builtin headers come
        # from the wrapper's resource-root instead.
        CC_wasm32_unknown_unknown = "${pkgs.llvmPackages_18.clang-unwrapped}/bin/clang";
        # bintools (the wrapper) only exposes unprefixed names (ar, ld…);
        # llvm-ar lives in bintools-unwrapped. The wrapper path only
        # "works" while a warm target/ keeps ring's build script from
        # re-running — cold CI builds hit it.
        AR_wasm32_unknown_unknown = "${pkgs.llvmPackages_18.bintools-unwrapped}/bin/llvm-ar";
        CFLAGS_wasm32_unknown_unknown = "-isystem ${pkgs.llvmPackages_18.clang}/resource-root/include";
        RUST_SRC_PATH = "${config.fts.rustToolchain}/lib/rustlib/src/rust/library";
      }
      // lib.optionalAttrs pkgs.stdenv.isLinux {
        LD_LIBRARY_PATH = libPath;
        XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}";
        # WebKitGTK accelerated compositing fails on NixOS (GBM buffer
        # error → white window). Force software rendering.
        # See: https://github.com/NixOS/nixpkgs/issues/32580
        WEBKIT_DISABLE_COMPOSITING_MODE = "1";
      }
      // lib.optionalAttrs pkgs.stdenv.isDarwin (
        let
          # iOS cross toolchain: the nix cc wrapper only targets macOS, so
          # the iOS targets link/compile through Xcode's clang via xcrun,
          # with the nix SDK env (SDKROOT/DEVELOPER_DIR) scrubbed.
          iosCC = sdk: pkgs.writeShellScript "ios-clang-${sdk}" ''
            exec /usr/bin/env -u SDKROOT -u DEVELOPER_DIR /usr/bin/xcrun --sdk ${sdk} clang "$@"
          '';
          iosCXX = sdk: pkgs.writeShellScript "ios-clang++-${sdk}" ''
            exec /usr/bin/env -u SDKROOT -u DEVELOPER_DIR /usr/bin/xcrun --sdk ${sdk} clang++ "$@"
          '';
          # The wasm C compiler (ring's build for the web bundle) is clang-18,
          # but the darwin DYLD_LIBRARY_PATH below forces it to load the
          # stdenv's newer libclang-cpp → symbol mismatch → SIGABRT. Run it
          # with DYLD_LIBRARY_PATH unset so it resolves its OWN libs via
          # rpath.
          wasmCC = pkgs.writeShellScript "wasm-clang-18" ''
            exec /usr/bin/env -u DYLD_LIBRARY_PATH ${pkgs.llvmPackages_18.clang-unwrapped}/bin/clang "$@"
          '';
        in
        {
          DYLD_LIBRARY_PATH = libPath;
          # Override the common wasm CC with the DYLD-clean wrapper (darwin only).
          CC_wasm32_unknown_unknown = "${wasmCC}";
          # Rust defaults aarch64-apple-ios to iOS 10, whose runtime lacks
          # `___chkstk_darwin` (it moved into libSystem at iOS 12) — linking
          # a large-stack crate then fails with an undefined symbol. Pin a
          # modern floor for compile + link consistency.
          IPHONEOS_DEPLOYMENT_TARGET = "15.0";
          CARGO_TARGET_AARCH64_APPLE_IOS_LINKER = "${iosCC "iphoneos"}";
          CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER = "${iosCC "iphonesimulator"}";
          CC_aarch64_apple_ios = "${iosCC "iphoneos"}";
          CXX_aarch64_apple_ios = "${iosCXX "iphoneos"}";
          CC_aarch64_apple_ios_sim = "${iosCC "iphonesimulator"}";
          CXX_aarch64_apple_ios_sim = "${iosCXX "iphonesimulator"}";
        }
      );
    };
}
