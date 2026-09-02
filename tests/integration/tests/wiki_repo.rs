//! Chapter — a wiki whose pages live in a git repository.
//!
//! `features/wiki/spec/wiki.md`, "Repo-sourced wikis". ACME's seed
//! carries a small `task-docs` repository, made a repository at plant
//! time, and a `Docs` wiki declared over its `docs/` folder. This
//! chapter reaches that wiki the way a person does — as Alice, over
//! ACME's org router — and asserts three things the spec promises:
//!
//! - **the repository is authoritative** (`wiki.source.repo`): the
//!   wiki lists as repo-sourced, names the commit it reflects, and its
//!   pages are the repository's files and nothing else;
//! - **the mirror tracks the repository** (`wiki.source.sync`): a
//!   commit upstream becomes a page after a refresh, without anyone
//!   importing anything;
//! - **where it came from changes nothing else**
//!   (`wiki.source.same-surface`): the same page lane reads it, and
//!   the same subscription lane copies it, as for any other wiki.
//!
//! The upstream commit is made with the real `git` binary against the
//! seeded repository on the server's disk, because "what git tooling
//! sees" is the criterion and only git can answer it. Skipped, with a
//! reason, where `git` is absent — the seed itself degrades the same
//! way.

use std::path::Path;
use std::process::Command;

use integration::scenario::Scenario;
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

/// The declared repo-sourced wiki the seed plants for ACME.
fn declared() -> &'static task_server::example_org::DeclaredRepoWiki {
    task_server::example_org::repo_wikis_of("acme-audio")
        .next()
        .expect("the example declares a repo-sourced wiki for ACME")
}

fn git_on_path() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `git` in `dir` as a documentation author, asserting success.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.email=sam@acme.test", "-c", "user.name=Sam"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// t[verify wiki.source.repo] — the seeded wiki lists as
/// repo-sourced, reflects the repository's HEAD, and holds exactly the
/// files under the mirrored path.
///
/// t[verify wiki.source.sync] — a commit upstream is a page after
/// `refresh_source`, and the reflected commit moves with it.
///
/// t[verify wiki.source.same-surface] — read through the same page
/// lane, subscribed through the same subscription lane, as any wiki.
#[tokio::test(flavor = "multi_thread")]
async fn a_repo_sourced_wiki_mirrors_its_repository_and_is_a_wiki_like_any_other() {
    if !git_on_path() {
        // The seed plants no repo-sourced wiki without git, so there
        // is nothing here to assert against.
        return;
    }
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let w = declared();
    let slug = task_server::example_org::repo_wiki_slug(w);
    let repo = s.orgs.acme.org_root().join("repos").join(w.repo);
    assert!(
        repo.join(".git").is_dir(),
        "the seed did not plant {} as a repository",
        repo.display()
    );
    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();

    // ── The repository is authoritative ──────────────────────────────
    let wikis = alice.wikis().await.list_wikis().await.expect("list wikis");
    let docs = wikis.iter().find(|x| x.slug == slug).unwrap_or_else(|| {
        panic!(
            "`{slug}` is not in {:?}",
            wikis.iter().map(|x| &x.slug).collect::<Vec<_>>()
        )
    });
    assert!(
        docs.repo_sourced,
        "the seeded Docs wiki must list as repo-sourced"
    );
    assert!(
        wikis.iter().any(|x| !x.repo_sourced),
        "the seed also holds ordinary wikis, so the flag distinguishes"
    );

    let described = alice
        .wikis()
        .await
        .describe_wiki(slug.clone())
        .await
        .expect("describe");
    let source = described
        .config
        .source
        .clone()
        .expect("a repo-sourced wiki declares its source");
    assert!(is_sha(&source.commit), "commit is not a sha: {source:?}");
    assert_eq!(
        source.commit, head,
        "the wiki reflects the repository's HEAD"
    );
    assert_eq!(source.path, w.path);
    assert!(
        source.last_error.is_empty(),
        "a fresh plant is not stale: {source:?}"
    );

    let pages = alice
        .wiki_pages()
        .await
        .list_pages(slug.clone())
        .await
        .expect("list pages");
    let listed: Vec<&str> = pages.iter().map(|p| p.path.as_str()).collect();
    for entry in std::fs::read_dir(repo.join(w.path)).expect("mirrored path") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".md") {
            assert!(
                listed.contains(&name.as_str()),
                "`{name}` not listed in {listed:?}"
            );
        }
    }
    assert!(
        !listed.contains(&"README.md"),
        "the README outside `{}` must not be a page: {listed:?}",
        w.path
    );

    // ── The mirror tracks the repository ─────────────────────────────
    std::fs::write(
        repo.join(w.path).join("Release Notes.md"),
        "---\ntitle: Release Notes\n---\n\n# Release Notes\n\nShipped with 1.2.\n",
    )
    .expect("write upstream");
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &["commit", "-q", "-m", "docs: release notes for 1.2"],
    );
    let new_head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(new_head, head);

    // Not yet: the wiki reflects what it last fetched.
    let before = alice
        .wiki_pages()
        .await
        .list_pages(slug.clone())
        .await
        .expect("list pages");
    assert!(
        !before.iter().any(|p| p.path == "Release Notes.md"),
        "a commit is not a page until the mirror fetches"
    );

    let refreshed = alice
        .wikis()
        .await
        .refresh_source(slug.clone())
        .await
        .expect("refresh source");
    assert_eq!(
        refreshed.commit, new_head,
        "the refresh followed the branch"
    );
    assert!(refreshed.last_error.is_empty());
    let after = alice
        .wiki_pages()
        .await
        .list_pages(slug.clone())
        .await
        .expect("list pages");
    assert!(
        after.iter().any(|p| p.path == "Release Notes.md"),
        "the new page is in the wiki after a refresh: {:?}",
        after.iter().map(|p| &p.path).collect::<Vec<_>>()
    );
    let page = alice
        .wiki_pages()
        .await
        .read_page(slug.clone(), "Release Notes.md".into())
        .await
        .expect("read page");
    assert!(
        page.markdown.contains("Shipped with 1.2."),
        "{}",
        page.markdown
    );

    // ── Where it came from changes nothing else ──────────────────────
    // The same subscription lane the seed uses for Music Theory copies
    // this wiki too: ACME's own vault takes it on, refreshes, and holds
    // the same number of files the repository's path has.
    let me = Subscriber::Vault;
    let qualified = format!("acme.test/{slug}");
    alice
        .wiki_subscriptions()
        .await
        .subscribe(
            me.clone(),
            Subscription {
                domain: "acme.test".into(),
                slug: slug.clone(),
                kind: SourceKind::Wiki,
                title: w.title.into(),
                core: false,
                declined: false,
            },
        )
        .await
        .expect("subscribe to the docs wiki");
    let report = alice
        .wiki_subscriptions()
        .await
        .refresh_subscription(me.clone(), qualified.clone())
        .await
        .expect("refresh the subscription");
    assert!(report.pulled > 0, "{report:?}");
    assert!(report.conflicted.is_empty(), "{report:?}");
    let held = alice
        .wiki_subscriptions()
        .await
        .list_subscriptions(me)
        .await
        .expect("list subscriptions");
    let entry = held
        .iter()
        .find(|h| h.subscription.slug == slug)
        .expect("the docs wiki is held");
    assert!(entry.files > 0, "the copy is on disk: {entry:?}");
    assert!(
        entry.files >= after.len() as u32,
        "the copy holds every page the wiki lists ({} < {})",
        entry.files,
        after.len()
    );
}
