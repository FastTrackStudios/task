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
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "schema",
            "bootstrap",
            "--wiki",
            "knowledge",
        ],
    );
    assert!(ok(&out).contains("bootstrapped knowledge"));

    // No verb assumes a wiki: without `--wiki` (or `TASK_WIKI`) the
    // flat command is refused and names the flag, rather than
    // answering from the `knowledge` tier as `"default"` used to.
    let out = task(tmp.path(), &["--org", "t", "wiki", "health"]);
    assert!(!out.status.success(), "a wiki must be named");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--wiki"), "{stderr}");
    assert!(stderr.contains("TASK_WIKI"), "{stderr}");

    // `TASK_WIKI` is the one default there is, and it is the caller's.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_task"))
        .args(["--org", "t", "wiki", "health"])
        .current_dir(tmp.path())
        .env("TASK_DATA_ROOT", tmp.path())
        .env("TASK_EMBED", "1")
        .env("TASK_WIKI", "knowledge")
        .env_remove("TASK_VOX_URL")
        .env("TASK_SESSION_FILE", tmp.path().join("session.json"))
        .output()
        .expect("spawn task binary");
    assert!(ok(&out).contains("bootstrapped:    true"));

    // The flat `wiki health`, with no `--vault`, answers over vox.
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "health", "--wiki", "knowledge"],
    );
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
        &[
            "--org",
            "t",
            "wiki",
            "schema",
            "bootstrap",
            "--wiki",
            "knowledge",
        ],
    ));

    // Import a local folder INTO the org wiki over vox.
    let src = tmp.path().join("incoming");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("note.md"), "# A note\n\nhello\n").unwrap();
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "import",
            "--dir",
            "incoming",
            "--wiki",
            "knowledge",
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("Imported 1 file(s)"), "{stdout}");
    assert!(stdout.contains("raw/sources/note.md"), "{stdout}");

    // Diff-only rescan (no --enqueue): sees the import, enqueues
    // nothing — the `rescan_diff` RPC keeps the contract.
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "rescan", "--wiki", "knowledge"],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("created=1"), "{stdout}");
    assert!(stdout.contains("+ raw/sources/note.md"), "{stdout}");
    assert!(!stdout.contains("enqueued"), "{stdout}");

    // Mutate the source in the org tree, rescan --enqueue → one
    // modified entry, one ingest task queued over vox.
    let on_disk = tmp.path().join("orgs/t/wiki/Knowledge/raw/sources/note.md");
    std::fs::write(&on_disk, "# A note\n\nhello again\n").unwrap();
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "rescan",
            "--enqueue",
            "--wiki",
            "knowledge",
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("modified=1"), "{stdout}");
    assert!(stdout.contains("enqueued 1 ingest task(s)"), "{stdout}");

    // Health over vox reflects the queued task; findings listing
    // answers (empty) over vox as well.
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "health", "--wiki", "knowledge"],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("queue_depth:     1"), "{stdout}");
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "findings", "--wiki", "knowledge"],
    );
    assert!(ok(&out).contains("Open findings: 0"));
}

/// `task wiki scaffold` over vox: a wiki from a purpose statement,
/// read back through the same services, idempotent on re-run, and its
/// root known to the FS-only verbs through `describe`.
#[test]
fn wiki_scaffold_over_embedded_vox() {
    let tmp = scratch_org();
    let purpose = "Notes and questions from a weekly study of the Gospel of John.";

    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "scaffold",
            "--title",
            "Bible Study",
            "--purpose",
            purpose,
            "--visibility",
            "unlisted",
            "--types",
            "topic,question,person",
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("created `bible-study`"), "{stdout}");
    assert!(stdout.contains("wrote   purpose.md"), "{stdout}");
    assert!(
        stdout.contains("wrote   schema.md (topic, question, person, source)"),
        "{stdout}"
    );
    assert!(stdout.contains("wrote   Goals.md"), "{stdout}");
    assert!(stdout.contains("rebuilt index.md"), "{stdout}");

    // The wiki is in the org's set, and `describe` says where it lives.
    let out = task(tmp.path(), &["--org", "t", "wiki", "list"]);
    let stdout = ok(&out);
    assert!(stdout.contains("bible-study"), "{stdout}");
    assert!(stdout.contains("unlisted"), "{stdout}");
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "describe", "bible-study", "--json"],
    );
    let described: serde_json::Value = serde_json::from_str(&ok(&out)).expect("json");
    assert_eq!(described["root"], "wikis/bible-study");
    assert!(
        tmp.path()
            .join("orgs/t/wikis/bible-study/purpose.md")
            .is_file(),
        "scaffolded on disk where describe says"
    );

    // Read back over vox: the purpose is a document, not a stub; the
    // schema declares the named types; Goals.md is a page.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "schema",
            "purpose",
            "--wiki",
            "bible-study",
        ],
    );
    let doc = ok(&out);
    assert!(doc.contains(purpose), "{doc}");
    assert!(doc.contains("## Who reads it"), "{doc}");
    assert!(doc.contains("## Out of scope"), "{doc}");
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "schema",
            "show",
            "--wiki",
            "bible-study",
        ],
    );
    let doc = ok(&out);
    assert!(doc.contains("| `person` | `People/` |"), "{doc}");
    assert!(doc.contains("[[bible::John.3.16]]"), "{doc}");
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "page",
            "read",
            "bible-study",
            "Goals.md",
        ],
    );
    assert!(ok(&out).contains("# Goals — Bible Study"));

    // The hidden alias still works for one release.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "schema",
            "health",
            "--wiki-id",
            "bible-study",
        ],
    );
    assert!(ok(&out).contains("schema_present: true"));

    // Re-running fills nothing: everything is kept.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "scaffold",
            "--title",
            "Bible Study",
            "--purpose",
            purpose,
        ],
    );
    let stdout = ok(&out);
    assert!(stdout.contains("`bible-study` exists"), "{stdout}");
    assert!(stdout.contains("kept    purpose.md"), "{stdout}");
    assert!(stdout.contains("kept    schema.md"), "{stdout}");
    assert!(stdout.contains("kept    Goals.md"), "{stdout}");
    assert!(!stdout.contains("wrote"), "{stdout}");

    // An FS-only verb resolves `--wiki` to the tree through the server
    // (embedded here, so the data root is this machine's): `context`
    // reads the scaffolded pages and reports what it saw.
    let out = task(
        tmp.path(),
        &["--org", "t", "wiki", "context", "", "--wiki", "bible-study"],
    );
    let stdout = ok(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[wiki context]"), "{stderr}");
    assert!(stdout.contains("Goals"), "{stdout}\n{stderr}");

    // A wiki the org does not hold is refused by name.
    let out = task(
        tmp.path(),
        &[
            "--org",
            "t",
            "wiki",
            "context",
            "",
            "--wiki",
            "no-such-wiki",
        ],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no-such-wiki"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
