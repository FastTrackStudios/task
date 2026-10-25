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
    let sources = [
        "ios/TaskLive/TaskLive.swift",
        "ios/Shared/TaskTimerAttributes.swift",
        "ios/Shared/OpenTaskIntent.swift",
    ];
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
        // `UIApplication` is the app's, not the extension's (OpenTaskIntent).
        .args(["-D", "TASK_APP"])
        .arg("-o")
        .arg(&lib)
        .args(sources)
        // The build machine's nix shell must not lend swiftc its SDK.
        .env_remove("SDKROOT")
        .env_remove("DEVELOPER_DIR")
        .status()
        .expect("run swiftc");
    assert!(status.success(), "swiftc failed for TaskLive");
    app_intents_metadata(&out, sdk, sdk_path.trim(), triple, &sources);
    println!("cargo:rustc-link-search=native={}", out.display());
    // Whole: nothing in Rust names `OpenTaskIntent`, and the system finds
    // it by type at run time, so the linker must not strip it.
    println!("cargo:rustc-link-lib=static:+whole-archive=TaskLive");
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
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,AppIntents");
}

const CONST_EXTRACT_PROTOCOLS: &str = r#"["AppIntent","EntityQuery","AppEntity","TransientEntity","AppEnum","AppShortcutProviding","AppShortcutsProvider","AnyResolverProviding","AppIntentsPackage","DynamicOptionsProvider","_IntentValueRepresentable","_AssistantIntentsProvider","_GenerativeFunctionExtractable"]"#;

/// The app's `Metadata.appintents`: what tells the system the app itself
/// can run `OpenTaskIntent` (Xcode makes it for an Xcode target; dx does
/// not). Written to `target/<triple>/<profile>/appintents/`, where
/// build-ios.sh and deploy-testflight.sh copy it into the bundle.
fn app_intents_metadata(
    out: &std::path::Path,
    sdk: &str,
    sdk_path: &str,
    triple: &str,
    sources: &[&str],
) {
    // The intents' constant values, read from the protocols Xcode lists in
    // `*_const_extract_protocols.json`. Only the frontend writes them
    // (the driver drops the flag), and a type check is enough.
    let protocols = out.join("const_extract_protocols.json");
    std::fs::write(&protocols, CONST_EXTRACT_PROTOCOLS).expect("write protocols list");
    let const_values = out.join("TaskLive.swiftconstvalues");
    let status = Command::new("xcrun")
        .args([
            "--sdk",
            sdk,
            "swift-frontend",
            "-typecheck",
            "-parse-as-library",
        ])
        .args([
            "-module-name",
            "TaskLive",
            "-target",
            triple,
            "-sdk",
            sdk_path,
        ])
        .args(["-D", "TASK_APP", "-emit-const-values-path"])
        .arg(&const_values)
        .arg("-const-gather-protocols-file")
        .arg(&protocols)
        .args(sources)
        .env_remove("SDKROOT")
        .env_remove("DEVELOPER_DIR")
        .status()
        .expect("run swift-frontend");
    assert!(
        status.success(),
        "const value extraction failed for TaskLive"
    );
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source_list = out.join("TaskLive.SwiftFileList");
    let list: String = sources
        .iter()
        .map(|s| format!("{}\n", manifest.join(s).display()))
        .collect();
    std::fs::write(&source_list, list).expect("write source list");
    let const_list = out.join("TaskLive.SwiftConstValuesFileList");
    std::fs::write(&const_list, format!("{}\n", const_values.display()))
        .expect("write const values list");
    let swiftc = run(Command::new("xcrun").args(["--find", "swiftc"]));
    let toolchain = PathBuf::from(swiftc.trim())
        .ancestors()
        .nth(3)
        .map(PathBuf::from)
        .expect("toolchain dir");
    let version = run(Command::new("xcrun").args(["xcodebuild", "-version"]));
    let build = version
        .lines()
        .find_map(|l| l.strip_prefix("Build version "))
        .unwrap_or("")
        .trim()
        .to_string();
    let staged = out.join("appintents");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).expect("create appintents dir");
    let status = Command::new("xcrun")
        .arg("appintentsmetadataprocessor")
        .arg("--toolchain-dir")
        .arg(&toolchain)
        .args(["--module-name", "TaskLive", "--sdk-root", sdk_path])
        .args(["--xcode-version", &build, "--platform-family", "iOS"])
        .args(["--deployment-target", "15.0", "--target-triple", triple])
        .arg("--source-file-list")
        .arg(&source_list)
        .arg("--swift-const-vals-list")
        .arg(&const_list)
        .arg("--output")
        .arg(&staged)
        .args([
            "--compile-time-extraction",
            "--no-app-shortcuts-localization",
        ])
        .env_remove("SDKROOT")
        .env_remove("DEVELOPER_DIR")
        .status()
        .expect("run appintentsmetadataprocessor");
    assert!(status.success(), "appintentsmetadataprocessor failed");
    // OUT_DIR is target/<triple>/<profile>/build/<pkg>-<hash>/out, or
    // build/<pkg>/<hash>/out under nightly cargo's new build-dir layout,
    // so the profile dir is the parent of `build`, not a fixed depth up.
    let profile_dir = out
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "build"))
        .and_then(|build| build.parent());
    if let Some(profile_dir) = profile_dir {
        let dest = profile_dir.join("appintents");
        let _ = std::fs::remove_dir_all(&dest);
        copy_dir(&staged, &dest);
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir").flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).expect("copy file");
        }
    }
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
