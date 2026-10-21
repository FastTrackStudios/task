//! `task-server --version` / `--help` answer and exit — they never boot.
//!
//! The bug this pins: `task-server --version` in the production pod
//! started the server (it logged "OTLP export enabled" and bound its
//! port) instead of printing a version, because every argument that was
//! not `admin` or `peer` fell through to the boot. These run the real
//! binary with an OTLP endpoint, a data root and a bind address all set,
//! and assert nothing of the server happened: the right line on stdout,
//! exit 0, an untouched (absent) data root, a port nobody bound, and no
//! log line on stderr.
//!
//! The version it prints must be the release's: `[workspace.package]
//! version` of the root Cargo.toml, the one version every artifact of a
//! release reports (scripts/check-versions.sh checks the rest).

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// `[workspace.package] version` from the root Cargo.toml, read the way
/// scripts/release.sh reads it.
fn workspace_version() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let text = std::fs::read_to_string(&root).unwrap();
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
            continue;
        }
        if in_section
            && let Some(rest) = line.strip_prefix("version")
            && let Some(v) = rest.trim().strip_prefix('=')
        {
            return v.trim().trim_matches('"').to_owned();
        }
    }
    panic!("no [workspace.package] version in {}", root.display());
}

/// Run the binary with everything set that would make a boot visible.
fn run(arg: &str, data_root: &Path, bind: &str) -> Output {
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_task-server"))
        .arg(arg)
        .env("TASK_DATA_ROOT", data_root)
        .env("TASK_SERVER_BIND", bind)
        // Would make a booting server log "OTLP export enabled".
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1")
        .env("RUST_LOG", "info")
        .env("TASK_IROH_DISABLE", "1")
        .output()
        .expect("run task-server");
    // A server never exits on its own; anything that returned is not one.
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "`task-server {arg}` took {:?}",
        started.elapsed()
    );
    out
}

fn free_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    drop(l);
    addr
}

fn assert_no_side_effects(arg: &str, out: &Output, data_root: &Path, bind: &str) {
    assert!(
        out.status.success(),
        "`task-server {arg}` exited {}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.trim().is_empty(),
        "`task-server {arg}` logged as if it booted:\n{stderr}"
    );
    assert!(
        !data_root.exists(),
        "`task-server {arg}` created its data root {}",
        data_root.display()
    );
    // The port is still free: nothing bound it (and nothing still holds it).
    TcpListener::bind(bind)
        .unwrap_or_else(|e| panic!("`task-server {arg}` left {bind} bound: {e}"));
}

#[test]
fn version_prints_the_workspace_version_and_exits_without_side_effects() {
    let want = format!("task-server {}", workspace_version());
    for arg in ["--version", "-V", "version"] {
        let tmp = tempfile::tempdir().unwrap();
        let data_root = tmp.path().join("data");
        let bind = free_port();
        let out = run(arg, &data_root, &bind);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            want,
            "`task-server {arg}`"
        );
        assert_no_side_effects(arg, &out, &data_root, &bind);
    }
}

#[test]
fn help_prints_usage_and_exits_without_side_effects() {
    for arg in ["--help", "-h"] {
        let tmp = tempfile::tempdir().unwrap();
        let data_root = tmp.path().join("data");
        let bind = free_port();
        let out = run(arg, &data_root, &bind);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("task-server peer sign-in"), "{stdout}");
        assert!(stdout.contains("--version"), "{stdout}");
        assert_no_side_effects(arg, &out, &data_root, &bind);
    }
}

#[test]
fn an_unknown_argument_is_refused_not_booted() {
    let tmp = tempfile::tempdir().unwrap();
    let data_root = tmp.path().join("data");
    let bind = free_port();
    let out = run("--verison", &data_root, &bind);
    assert_eq!(out.status.code(), Some(2), "a typo must not start a server");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown argument `--verison`"));
    assert!(!data_root.exists());
    TcpListener::bind(&bind).unwrap();
}

#[test]
fn service_show_renders_the_unit_without_touching_the_node() {
    let tmp = tempfile::tempdir().unwrap();
    let data_root = tmp.path().join("data");
    let out = Command::new(env!("CARGO_BIN_EXE_task-server"))
        .args([
            "service",
            "show",
            "task-server.service",
            "--for",
            "systemd",
            "--data-root",
        ])
        .arg(&data_root)
        .env("TASK_DATA_ROOT", &data_root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let unit = String::from_utf8_lossy(&out.stdout);
    assert!(
        unit.contains("ExecStart=/usr/local/bin/task-server peer"),
        "{unit}"
    );
    assert!(
        unit.contains(&format!("TASK_DATA_ROOT={}", data_root.display())),
        "{unit}"
    );
    assert!(!data_root.exists());
}
