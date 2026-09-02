//! CLI-level proof that commands reach org data OVER VOX with no
//! running server: `TASK_EMBED=1` boots `task_server::AppState`
//! in-process (see `establish_client` in `main.rs`) and the same
//! typed clients talk to it over architect's local transport.
//!
//! Each test spawns the real `task` binary in a scratch
//! `TASK_DATA_ROOT`, so nothing races env vars in-process and the
//! tests parallelize safely.
//!
//! The flat wiki commands run with no `--vault`, which is what
//! routes them to the org wiki over vox: the FS path is the escape
//! hatch and the org is the default.
//!
//! That used to depend on the working directory. `--vault`
//! defaulted to `examples/vault`, so what selected the vox path was
//! that directory failing to resolve from the scratch cwd — the
//! test passed because of where it ran rather than what it asked
//! for, and it would have started querying a local tree the moment
//! anyone ran it from the repo root. The flag is optional now and
//! the routing says what it means.

use std::path::Path;
use std::process::Output;

/// Run the `task` binary against `data_root` in embedded mode.
fn task(data_root: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_task"))
        .args(args)
        .current_dir(data_root)
        .env("TASK_DATA_ROOT", data_root)
        .env("TASK_EMBED", "1")
        // Ambient dev-machine config must not leak in.
        .env_remove("TASK_VOX_URL")
        .env_remove("TASK_SESSION_TOKEN")
        .env_remove("TASK_ORGANIZATION_ID")
        .env_remove("TASK_SERVER_WIKI_ROOT")
        .env_remove("TASK_SERVER_ORG")
        .env_remove("TASK_SERVER_VAULT_ROOT")
        .env_remove("TASK_TIMER_DB")
        .env_remove("TASK_SERVER_TIMER_URL")
        .env_remove("TASK_ORG_ID")
        .env_remove("TASK_USER_ID")
        .env_remove("TASK_VAULT_ROOT")
        // Keep sign-ins inside the scratch root, away from the
        // developer's real ~/.local/share/task/session.json.
        .env("TASK_SESSION_FILE", data_root.join("session.json"))
        .output()
        .expect("spawn task binary")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Scaffold an org named `t` in a fresh data root.
fn scratch_org() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = task(tmp.path(), &["org", "init", "t", "--name", "T"]);
    ok(&out);
    tmp
}

#[test]
fn auth_signup_and_users_over_embedded_vox() {
    let tmp = scratch_org();

    // Empty org, no session: the listing is REFUSED, not answered.
    // `AuthService::list_org_members` requires a session that validates
    // against this org — the tokenless enumerate-everything fallback
    // used to hand every user's name and email to anonymous callers
    // (found open on production 2026-08-08). It is still a vox call,
    // not a "local-only command" refusal: the error is the service's.
    let out = task(tmp.path(), &["--org", "t", "auth", "users"]);
    assert!(!out.status.success(), "a tokenless listing must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("permission denied"),
        "expected the service's refusal, got:\n{stderr}"
    );

    // Sign up over the embedded org AuthService, then list again.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "auth",
            "signup",
            "--email",
            "a@example.com",
            "--password",
            "hunter22",
            "--name",
            "Alice",
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("Created user a@example.com"), "{stdout}");

    let out = task(tmp.path(), &["--org", "t", "auth", "users"]);
    let stdout = ok(&out);
    assert!(stdout.contains("a@example.com"), "{stdout}");

    // And the session landed in the scratch session file, not the
    // developer's real one.
    assert!(tmp.path().join("session.json").is_file());
}

#[test]
fn timer_lifecycle_over_embedded_vox() {
    let tmp = scratch_org();

    // start → active → stop, plus a --tag that exercises the new
    // TimerService tag RPCs. The CLI opens no timer.sqlite itself —
    // everything goes through the embedded backend.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "timer",
            "start",
            "hello world",
            "--tag",
            "focus",
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("Started "), "{stdout}");
    assert!(stdout.contains("description: hello world"), "{stdout}");
    assert!(stdout.contains("tags:        focus"), "{stdout}");

    let out = task(tmp.path(), &["--org", "t", "timer", "active"]);
    assert!(ok(&out).contains("Running for"));

    let out = task(tmp.path(), &["--org", "t", "timer", "stop"]);
    assert!(ok(&out).contains("Stopped "));

    let out = task(tmp.path(), &["--org", "t", "timer", "list"]);
    assert!(ok(&out).contains("hello world"));

    let out = task(tmp.path(), &["--org", "t", "timer", "tag", "list"]);
    assert!(ok(&out).contains("focus"));

    // The rows really landed in the org's timer.sqlite — written by
    // the embedded server, not by a CLI-side connection.
    assert!(tmp.path().join("orgs/t/timer.sqlite").is_file());
}

#[test]
fn finance_reports_over_embedded_vox() {
    let tmp = scratch_org();

    // Retro-log a closed session, then pull the finance rollups —
    // sessions arrive via TimerService, invoices via Invoicing.
    let now = chrono::Utc::now();
    let from = (now - chrono::Duration::hours(2)).to_rfc3339();
    let to = (now - chrono::Duration::hours(1)).to_rfc3339();
    let out = task(
        tmp.path(),
        &[
            "--org", "t", "timer", "log", "editing", "--from", &from, "--to", &to,
        ],
    );
    assert!(ok(&out).contains("Logged "));

    let out = task(tmp.path(), &["--org", "t", "finance", "project"]);
    let stdout = ok(&out);
    assert!(stdout.contains("(unscoped)"), "{stdout}");
    assert!(stdout.contains("sessions: 1"), "{stdout}");

    let out = task(tmp.path(), &["--org", "t", "finance", "weekly"]);
    assert!(ok(&out).contains("Time tracked — week of"));

    let out = task(tmp.path(), &["--org", "t", "finance", "invoices"]);
    assert!(ok(&out).contains("(no invoices)"));
}

#[test]
fn vault_mirror_over_embedded_vox() {
    let tmp = scratch_org();

    // Seed the ORG vault (the tree the embedded server serves).
    let vault = tmp.path().join("orgs/t/vault");
    std::fs::create_dir_all(vault.join("Notes")).unwrap();
    std::fs::write(vault.join("Notes/a.md"), "# A\n\nhello vault\n").unwrap();

    // A missing positional path routes to the org vault over vox
    // (mirror + identical FS logic) instead of erroring.
    let out = task(tmp.path(), &["--org", "t", "vault", "pages", "no-such-dir"]);
    assert!(ok(&out).contains("Notes/a.md"));

    let out = task(
        tmp.path(),
        &["--org", "t", "vault", "cat", "no-such-dir", "Notes/a.md"],
    );
    assert!(ok(&out).contains("hello vault"));

    // A mutation on the vox route pushes back into the org vault.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "vault",
            "create",
            "no-such-dir",
            "Notes/b.md",
            "--body",
            "created over vox",
        ],
    );
    ok(&out);
    let created = std::fs::read_to_string(vault.join("Notes/b.md")).expect("pushed back");
    assert!(created.contains("created over vox"));

    // --fs pins the old direct behaviour: the missing path is
    // walked as an (empty) on-disk vault, never routed to vox.
    let out = task(
        tmp.path(),
        &["--org", "t", "vault", "--fs", "pages", "no-such-dir"],
    );
    assert!(
        !ok(&out).contains("Notes/a.md"),
        "--fs must not fall back to vox"
    );
}

#[test]
fn wiki_bootstrap_and_health_over_embedded_vox() {
    let tmp = scratch_org();

    // Bootstrap the org wiki over vox (embedded server).
    let out = task(tmp.path(), &["--org", "t", "wiki", "schema", "bootstrap"]);
    assert!(ok(&out).contains("bootstrapped default"));

    // The flat `wiki health`, with no `--vault`, answers over vox.
    let out = task(tmp.path(), &["--org", "t", "wiki", "health"]);
    let stdout = ok(&out);
    assert!(
        stdout.contains("bootstrapped:    true"),
        "health over vox: {stdout}"
    );
    assert!(stdout.contains("schema_present:  true"), "{stdout}");

    // And the wiki really landed inside the org tree the embedded
    // server hosts.
    assert!(
        tmp.path().join("orgs/t/wiki/Knowledge/schema.md").is_file(),
        "org wiki scaffolded on disk"
    );
}

#[test]
fn wiki_import_rescan_findings_over_embedded_vox() {
    let tmp = scratch_org();
    ok(&task(
        tmp.path(),
        &["--org", "t", "wiki", "schema", "bootstrap"],
    ));

    // Import a local folder INTO the org wiki over vox.
    let src = tmp.path().join("incoming");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("note.md"), "# A note\n\nhello\n").unwrap();
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "import", "--dir", "incoming"],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("Imported 1 file(s)"), "{stdout}");
    assert!(stdout.contains("raw/sources/note.md"), "{stdout}");

    // Diff-only rescan (no --enqueue): sees the import, enqueues
    // nothing — the `rescan_diff` RPC keeps the contract.
    let out = task(tmp.path(), &["--org", "t", "wiki", "rescan"]);
    let stdout = ok(&out);
    assert!(stdout.contains("created=1"), "{stdout}");
    assert!(stdout.contains("+ raw/sources/note.md"), "{stdout}");
    assert!(!stdout.contains("enqueued"), "{stdout}");

    // Mutate the source in the org tree, rescan --enqueue → one
    // modified entry, one ingest task queued over vox.
    let on_disk = tmp.path().join("orgs/t/wiki/Knowledge/raw/sources/note.md");
    std::fs::write(&on_disk, "# A note\n\nhello again\n").unwrap();
    let out = task(tmp.path(), &["--org", "t", "wiki", "rescan", "--enqueue"]);
    let stdout = ok(&out);
    assert!(stdout.contains("modified=1"), "{stdout}");
    assert!(stdout.contains("enqueued 1 ingest task(s)"), "{stdout}");

    // Health over vox reflects the queued task; findings listing
    // answers (empty) over vox as well.
    let out = task(tmp.path(), &["--org", "t", "wiki", "health"]);
    let stdout = ok(&out);
    assert!(stdout.contains("queue_depth:     1"), "{stdout}");
    let out = task(tmp.path(), &["--org", "t", "wiki", "findings"]);
    assert!(ok(&out).contains("Open findings: 0"));
}
