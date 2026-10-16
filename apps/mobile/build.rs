//! Builds the app's Swift half (`ios/TaskLive` + `ios/Shared`) into a
//! static library and links it, on iOS only — ActivityKit (the timer's
//! Live Activity) has no Objective-C API, so a few `@_cdecl` functions
//! in Swift are what `src/live.rs` calls. Elsewhere this does nothing.
//!
//! Needs the real `xcrun` first on PATH (build-ios.sh and
//! deploy-testflight.sh arrange that for every iOS build).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=ios/TaskLive");
    println!("cargo:rerun-if-changed=ios/Shared");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios") {
        return;
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    let simulator = target.ends_with("-sim") || target.starts_with("x86_64");
    let (sdk, triple, swift_dir) = if simulator {
        (
            "iphonesimulator",
            "arm64-apple-ios15.0-simulator",
            "iphonesimulator",
        )
    } else {
        ("iphoneos", "arm64-apple-ios15.0", "iphoneos")
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let sdk_path = run(Command::new("xcrun").args(["--sdk", sdk, "--show-sdk-path"]));
    let lib = out.join("libTaskLive.a");
    let status = Command::new("xcrun")
        .args(["--sdk", sdk, "swiftc"])
        .args(["-parse-as-library", "-emit-library", "-static", "-O"])
        .args([
            "-module-name",
            "TaskLive",
            "-target",
            triple,
            "-sdk",
            sdk_path.trim(),
        ])
        .arg("-o")
        .arg(&lib)
        .arg("ios/TaskLive/TaskLive.swift")
        .arg("ios/Shared/TaskTimerAttributes.swift")
        // The build machine's nix shell must not lend swiftc its SDK.
        .env_remove("SDKROOT")
        .env_remove("DEVELOPER_DIR")
        .status()
        .expect("run swiftc");
    assert!(status.success(), "swiftc failed for TaskLive");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=TaskLive");
    // The Swift runtime: the OS's own, plus the toolchain's back-deploy
    // shims the objects autolink.
    let toolchain = run(Command::new("xcrun").args(["--find", "swiftc"]));
    let usr = PathBuf::from(toolchain.trim())
        .parent()
        .and_then(|b| b.parent())
        .map(PathBuf::from);
    if let Some(usr) = usr {
        println!(
            "cargo:rustc-link-search=native={}",
            usr.join("lib/swift").join(swift_dir).display()
        );
    }
    println!(
        "cargo:rustc-link-search=native={}/usr/lib/swift",
        sdk_path.trim()
    );
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Not on iOS 15: weak, so the app still launches there.
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,ActivityKit");
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,WidgetKit");
}

fn run(cmd: &mut Command) -> String {
    let out = cmd
        .env_remove("SDKROOT")
        .env_remove("DEVELOPER_DIR")
        .output()
        .expect("run xcrun");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}
