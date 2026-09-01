//! `admin demo` plants a world the app can stand on — asserted, not
//! assumed.
//!
//! The demo is the seeded half of the repo's representation policy
//! (every feature lives in the suite AND in the planted world), and
//! this is the suite's grip on it: the declared projects become File
//! Roots named after themselves (what the review surface mounts on),
//! the video deliverables are generated INSIDE those roots, and a
//! replant upgrades in place rather than duplicating.
//!
//! The plant runs as a SUBPROCESS (the real `admin demo` CLI), not as
//! an in-process call: the seeder's own `FilesBackend` keeps its
//! version stores open for the life of its process, and a second
//! backend over the same root in the same process deadlocks on the
//! store lock the moment it walks history (`chain`). The subprocess is
//! also simply more honest — it is exactly what `just demo` runs.

use files::service::roots::RootsService;

/// Run `task-server admin demo --org <slug>` against `data_root`.
fn plant(data_root: &std::path::Path, slug: &str) -> eyre::Result<()> {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_task-server"))
        .args(["admin", "demo", "--org", slug])
        .env("TASK_DATA_ROOT", data_root)
        .stdout(std::process::Stdio::null())
        .status()?;
    eyre::ensure!(status.success(), "admin demo --org {slug} failed");
    Ok(())
}

fn ffmpeg_on_path() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Multi-thread on purpose: planting checkpoints roots, and the Files
// lane parks blocking work that must not starve a lone runtime worker
// (the CLI runs on the default multi-thread runtime too).
#[tokio::test(flavor = "multi_thread")]
async fn demo_plants_adopted_roots_with_video_deliverables() -> eyre::Result<()> {
    let tmp = tempfile::tempdir()?;
    let slug = "acme-audio";
    plant(tmp.path(), slug)?;

    // SAFETY: single-test binary; only the DataRoot construction below
    // reads it.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_SERVER_VAULT_ROOT");
        std::env::remove_var("TASK_SERVER_ORG");
    }
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let org = root.org(slug);
    let backend = files::FilesBackend::new(org.path().join("files"), org.vault_dir())
        .map_err(|e| eyre::eyre!("files backend: {e}"))?;

    // Every declared project is an adopted root, named after itself —
    // the name is the convention the project page resolves its
    // deliverables through (`locate_in_root`).
    let roots = RootsService::list(&backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    for declared in task_server::example_org::declared_of(slug) {
        assert!(
            roots.iter().any(|r| r.name == declared.title),
            "{}: not adopted as a root (have: {:?})",
            declared.title,
            roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
        );
    }

    // Video deliverables live INSIDE the project's root — the review
    // player streams renditions from a root, so `resources/` would be
    // the wrong side of the boundary. Generated, so only asserted when
    // the machine can generate (CI's dev shell carries ffmpeg; a bare
    // machine gets the honest outstanding state instead).
    if ffmpeg_on_path() {
        for declared in task_server::example_org::declared_of(slug) {
            for (name, medium, _, _) in declared.deliverables {
                if *medium != project::Medium::Video {
                    continue;
                }
                let dest = org
                    .path()
                    .join("files")
                    .join("Projects")
                    .join(declared.dir)
                    .join("Deliverables")
                    .join(format!("{name}.mp4"));
                assert!(
                    dest.is_file(),
                    "{}: \"{name}\" not generated at {}",
                    declared.title,
                    dest.display()
                );
            }
        }
    }

    // Audio masters are delivered into the root too (the review
    // surface's waveform lane), alongside their `resources/` song copy
    // (the global player's lane). Same bytes, two conventions.
    for declared in task_server::example_org::declared_of(slug) {
        for (name, medium, scope, _) in declared.deliverables {
            if *medium != project::Medium::Audio || *scope != project::Scope::WholeProject {
                continue;
            }
            let dest = org
                .path()
                .join("files")
                .join("Projects")
                .join(declared.dir)
                .join("Deliverables")
                .join(format!("{name}.wav"));
            assert!(
                dest.is_file(),
                "{}: \"{name}\" not delivered at {}",
                declared.title,
                dest.display()
            );
        }
    }

    // A fresh video arrives as HISTORY, not a file: rough cut
    // checkpointed, final rendered over it, checkpointed again — the
    // review screen's version switcher and compare depend on there
    // being at least two commits to show.
    if ffmpeg_on_path() {
        for declared in task_server::example_org::declared_of(slug) {
            for (name, medium, _, _) in declared.deliverables {
                if *medium != project::Medium::Video {
                    continue;
                }
                let root = roots
                    .iter()
                    .find(|r| r.name == declared.title)
                    .expect("adopted above");
                let chain = files::FilesService::chain(
                    &backend,
                    root.id,
                    format!("Deliverables/{name}.mp4"),
                )
                .await
                .unwrap_or_else(|e| panic!("chain of \"{name}\": {e}"));
                assert!(
                    chain.len() >= 2,
                    "{}: \"{name}\" has {} version(s) on record — the demo \
                     promises a rough cut AND a final",
                    declared.title,
                    chain.len()
                );
            }
        }
    }

    // Replanting is an upgrade, not a duplication: same roots, same
    // count. (This is the path an old demo root takes to gain whatever
    // the seeder has since learned.)
    plant(tmp.path(), slug)?;
    let again = RootsService::list(&backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    assert_eq!(
        roots.len(),
        again.len(),
        "replant changed the root count: {:?}",
        again.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}

/// The seeded booking the finance integration is written against.
///
/// The repo's policy is that a feature lives in the suite *and* in the
/// planted world, and an integration is the case where that matters
/// most: `bookings-ui` decides in a unit test that a **completed**
/// booking gets an "Invoice…" action, and this is what proves there is
/// a completed booking to see it on. Without this, a demo user turns
/// finance on and finds nothing different anywhere.
///
/// Asserted through the scheduler that the app reads, not by looking at
/// the file — a booking that parses is the claim, not a file that
/// exists.
#[cfg(feature = "plugin-scheduling")]
#[tokio::test(flavor = "multi_thread")]
async fn the_planted_org_has_a_completed_booking_to_invoice() -> eyre::Result<()> {
    use scheduling::VaultScheduler;
    use scheduling_proto::BookingStatus;
    use scheduling_proto::service::bookings::Bookings;
    use scheduling_proto::service::event_types::EventTypes;

    let tmp = tempfile::tempdir()?;
    let slug = "acme-audio";
    plant(tmp.path(), slug)?;

    let root = org_proto::DataRoot::new(tmp.path().to_path_buf());
    let scheduler = VaultScheduler::new(root.org(slug).vault_dir())
        .map_err(|e| eyre::eyre!("scheduler: {e}"))?;

    let bookings = scheduler
        .list_bookings()
        .map_err(|e| eyre::eyre!("list bookings: {e}"))?;
    let done = bookings
        .iter()
        .find(|b| matches!(b.status, BookingStatus::Completed))
        .ok_or_else(|| {
            eyre::eyre!(
                "no completed booking in the planted org — the finance \
                 integration has nothing to appear on ({} booking(s) planted)",
                bookings.len()
            )
        })?;

    // The three things the integration hands to whoever bills it. A
    // booking with no attendee would produce an "Invoice…" link that
    // says who nothing.
    assert!(
        !done.attendee_name.trim().is_empty(),
        "the completed booking names nobody to bill"
    );

    // And the event type it points at, since that is where the link
    // gets what the work *was* and how long it took.
    let types = scheduler
        .list_event_types()
        .map_err(|e| eyre::eyre!("list event types: {e}"))?;
    let et = types
        .iter()
        .find(|et| et.id == done.event_type_id)
        .ok_or_else(|| {
            eyre::eyre!(
                "the completed booking points at event type {:?}, which is \
                 not planted — the invoice link would say \"Booking\" and 0 min",
                done.event_type_id
            )
        })?;
    assert!(!et.title.trim().is_empty(), "the event type has no title");
    assert!(et.duration_min > 0, "the event type has no duration");
    Ok(())
}

/// The named wikis land where a reference expects to find them.
///
/// t[verify wiki.many.set] — planting an org produces the *set* of
/// wikis it declares, each at `wikis/<slug>/`, and the slug on disk is
/// the one a reference carries (`alice.test/cooking::…`). A mismatch
/// here would surface only as a link that quietly fails to resolve,
/// which is exactly the failure the suite exists to catch early.
///
/// The Bible is deliberately NOT asserted: it downloads, and the suite
/// must pass on a machine with no network. `TASK_DEMO_NO_BIBLE` keeps
/// this test off the wire entirely.
#[tokio::test(flavor = "multi_thread")]
async fn demo_plants_the_orgs_named_wikis() -> eyre::Result<()> {
    let tmp = tempfile::tempdir()?;
    let data_root = tmp.path();
    // SAFETY: nextest runs one process per test, so this env write is
    // not racing another test in this binary.
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    plant(data_root, "alice-personal")?;

    let org = org_proto::DataRoot::new(data_root.to_owned()).org("alice-personal");

    let declared: Vec<String> = task_server::example_org::wikis_of("alice-personal")
        .map(|w| org_proto::wiki_slug(w.title))
        .collect();
    assert!(!declared.is_empty(), "the example declares personal wikis");

    let planted: Vec<String> = org.named_wikis().into_iter().map(|(s, _)| s).collect();
    for slug in &declared {
        assert!(
            planted.contains(slug),
            "declared wiki `{slug}` is not in the planted set {planted:?}"
        );
        assert!(
            org.named_wiki_dir(slug).join("purpose.md").is_file(),
            "`{slug}` planted without its purpose.md"
        );
    }

    // The default tier is a member of the set rather than a fourth
    // shape beside it.
    assert!(planted.iter().any(|s| s == org_proto::DEFAULT_WIKI));

    // Recipes reach the cookbook through the Cooking wiki, which is
    // what makes them shareable rather than a per-org store.
    assert!(
        org.named_wiki_dir("cooking")
            .join("Cookbook")
            .join("Sourdough Loaf.cook")
            .is_file(),
        "the Cooking wiki's cookbook did not plant"
    );

    // A replant tops up rather than duplicating.
    plant(data_root, "alice-personal")?;
    let again: Vec<String> = org.named_wikis().into_iter().map(|(s, _)| s).collect();
    assert_eq!(planted, again, "a replant changed the set of wikis");
    Ok(())
}

fn git_on_path() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The repo-sourced wiki is planted over a real repository.
///
/// t[verify wiki.source.repo] — the seed's `Repos/task-docs/` becomes
/// a git repository with a commit on `main`, and the `Docs` wiki
/// mirrors its `docs/` — every markdown file there is a page, the
/// README outside it is not, and the wiki's config names the commit it
/// reflects. A replant leaves the commit alone rather than committing
/// again over an unchanged tree.
///
/// Skipped with a warning where `git` is absent, the way the video
/// deliverables are where `ffmpeg` is: the plant degrades, it does not
/// fail, and this test asserts the degraded shape too.
#[tokio::test(flavor = "multi_thread")]
async fn demo_plants_a_repo_sourced_wiki_over_a_seeded_repository() -> eyre::Result<()> {
    let tmp = tempfile::tempdir()?;
    let data_root = tmp.path();
    // SAFETY: nextest runs one process per test.
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    plant(data_root, "acme-audio")?;

    let org = org_proto::DataRoot::new(data_root.to_owned()).org("acme-audio");
    let declared: Vec<_> = task_server::example_org::repo_wikis_of("acme-audio").collect();
    assert!(
        !declared.is_empty(),
        "the example declares a repo-sourced wiki for ACME"
    );

    if !git_on_path() {
        // No git: the plant degrades — everything else lands and the
        // repo-sourced wiki is simply absent rather than half-made.
        for w in &declared {
            let slug = task_server::example_org::repo_wiki_slug(w);
            assert!(
                !org.named_wiki_dir(&slug).exists(),
                "`{slug}` planted without git?"
            );
        }
        return Ok(());
    }

    for w in &declared {
        let slug = task_server::example_org::repo_wiki_slug(w);
        let repo = task_server::example_org::repo_path(&org, w);
        assert!(
            repo.join(".git").is_dir(),
            "{} is not a git repository",
            repo.display()
        );
        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()?;
        let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
        assert_eq!(
            head.len(),
            40,
            "no commit on the seeded repository: {head:?}"
        );

        let wiki = org.named_wiki_dir(&slug);
        let config = wiki_live::config::load(&wiki, &slug)?;
        let source = config.source.expect("the wiki declares its repository");
        assert_eq!(
            source.commit, head,
            "the wiki reflects the repository's HEAD"
        );
        assert_eq!(source.branch, task_server::example_org::REPO_BRANCH);
        assert_eq!(source.path, w.path);
        assert!(source.last_error.is_empty(), "{source:?}");
        assert!(!source.fetched_at.is_empty());
        assert_eq!(config.visibility, wiki_proto::config::Visibility::Public);

        // Every markdown file under the mirrored path is a page; the
        // README outside it is not.
        let mut mirrored = 0;
        for entry in std::fs::read_dir(repo.join(w.path))? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "md") {
                let page = wiki.join(entry.file_name());
                assert!(page.is_file(), "{} was not mirrored", page.display());
                assert_eq!(std::fs::read(&page)?, std::fs::read(entry.path())?);
                mirrored += 1;
            }
        }
        assert!(
            mirrored >= 3,
            "the seeded docs are too thin to demonstrate anything"
        );
        assert!(
            !wiki.join("README.md").exists(),
            "only `{}` is mirrored",
            w.path
        );
        assert!(
            wiki.join("purpose.md").is_file(),
            "a wiki says what it is for"
        );

        // The clone is beside the wikis, hidden, and not a wiki.
        let listed: Vec<String> = org.named_wikis().into_iter().map(|(s, _)| s).collect();
        assert!(
            listed.contains(&slug),
            "`{slug}` is not in the planted set {listed:?}"
        );
        assert!(
            !listed.iter().any(|s| s.contains("repos")),
            "the clone directory leaked into the wiki set: {listed:?}"
        );

        // A replant changes nothing: same commit, no second commit.
        plant(data_root, "acme-audio")?;
        let again = wiki_live::config::load(&wiki, &slug)?
            .source
            .expect("source kept");
        assert_eq!(
            again.commit, head,
            "a replant re-committed an unchanged repository"
        );
    }
    Ok(())
}

/// The references in the seeded pages resolve — against a reader's own
/// subscriptions, which is the whole of `wiki.subscribe.resolution`.
///
/// This is the test the seed existed for before any of it worked: the
/// committed pages were written in ADR 0002's format from the start,
/// so they were a failing test waiting for a resolver.
#[tokio::test(flavor = "multi_thread")]
async fn seeded_references_resolve_against_the_readers_subscriptions() -> eyre::Result<()> {
    use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

    let tmp = tempfile::tempdir()?;
    let data_root = tmp.path();
    // SAFETY: nextest runs one process per test.
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    plant(data_root, "acme-audio")?;

    let org = org_proto::DataRoot::new(data_root.to_owned()).org("acme-audio");
    let page = org
        .named_wiki_dir("audio-production")
        .join("Concepts")
        .join("Equalization.md");
    let markdown = std::fs::read_to_string(&page)?;

    // Audio Production cites Music Theory, including a block anchor.
    let found = wiki_proto::reference::scan(&markdown);
    let cross: Vec<_> = found
        .iter()
        .map(|(_, r)| r)
        .filter(|r| r.source.as_deref() == Some("music-theory"))
        .collect();
    eyre::ensure!(
        cross.len() >= 2,
        "the seeded page should cite Music Theory more than once"
    );
    eyre::ensure!(
        cross
            .iter()
            .any(|r| r.anchor.as_deref() == Some("partials")),
        "the block-anchor citation is what `wiki.ref.block` is for"
    );

    let store = wiki_live::subscriptions::SubscriptionStore::open(org.path());
    let reader = Subscriber::Vault;

    // A reader holding nothing: unresolved, and told which source —
    // never "no such page".
    for r in &cross {
        let err = wiki_proto::resolve(r, &[]).expect_err("nothing is held yet");
        assert_eq!(
            err,
            wiki_proto::Unresolved::NoSubscription("acme.test/music-theory".into())
        );
    }

    // The same reader, now subscribed: the same bytes resolve.
    store.subscribe(
        &reader,
        Subscription {
            domain: "acme.test".into(),
            slug: "music-theory".into(),
            kind: SourceKind::Wiki,
            title: "Music Theory".into(),
            core: false,
            declined: false,
        },
    )?;
    let held = store.active(&reader)?;
    for r in &cross {
        let hit = wiki_proto::resolve(r, &held)
            .expect("resolves now")
            .expect("not a local reference");
        assert_eq!(hit.via.slug, "music-theory");
        // And the page it names is really there.
        let target = org
            .named_wiki_dir("music-theory")
            .join("Concepts")
            .join(format!("{}.md", hit.target));
        assert!(
            target.is_file(),
            "reference names `{}`, which the seed does not contain",
            hit.target
        );
    }

    // Core arrived without anyone asking, and is a Resource.
    let core = store.list(&reader)?;
    let bible = core
        .iter()
        .find(|s| s.slug == "bible")
        .expect("scripture is core");
    assert!(bible.core && !bible.declined);
    assert!(!bible.kind.is_editable(), "a Resource is never editable");
    Ok(())
}

/// A subscription end to end: alice-personal takes on ACME's Music
/// Theory wiki, materializes it, and a reference written in ACME's own
/// prose then resolves to a file on alice's disk.
///
/// Two orgs on one data root, which is the arrangement `admin seed`
/// uses. The upstream side is the real vault sync backend over ACME's
/// wiki directory — the same engine the product syncs vaults with, and
/// the reason no second replication path had to be written.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_materializes_and_its_references_resolve() -> eyre::Result<()> {
    use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

    let tmp = tempfile::tempdir()?;
    let data_root_path = tmp.path();
    // SAFETY: nextest runs one process per test.
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    plant(data_root_path, "acme-audio")?;
    plant(data_root_path, "alice-personal")?;

    let data_root = org_proto::DataRoot::new(data_root_path.to_owned());
    let acme = data_root.org("acme-audio");
    let alice = data_root.org("alice-personal");

    // ACME serves its Music Theory wiki. A wiki is a vault, so the
    // vault sync backend mounts it unchanged.
    let upstream = vault::Backend::single("music-theory", acme.named_wiki_dir("music-theory"))?;

    let subscription = Subscription {
        domain: "acme.test".into(),
        slug: "music-theory".into(),
        kind: SourceKind::Wiki,
        title: "Music Theory".into(),
        core: false,
        declined: false,
    };
    let store = wiki_live::subscriptions::SubscriptionStore::open(alice.path());
    store.subscribe(&Subscriber::Vault, subscription.clone())?;

    let out = wiki_live::materialize::refresh_subscription(&upstream, alice.path(), &subscription)?;
    eyre::ensure!(out.pulled > 0, "a fresh subscription pulls the source");
    assert!(!out.has_local_work());

    // The copy is where a reference says it is, and it is markdown a
    // person could open.
    let copy = wiki_live::materialize::local_copy_dir(alice.path(), "acme.test", "music-theory");
    let ionian = copy.join("Concepts").join("Ionian.md");
    assert!(ionian.is_file(), "the subscribed page is on alice's disk");
    assert!(std::fs::read_to_string(&ionian)?.contains("major"));

    // Now the payoff: a reference ACME wrote resolves for alice,
    // because alice holds the source — and lands on a real file.
    let acme_page = std::fs::read_to_string(
        acme.named_wiki_dir("audio-production")
            .join("Concepts")
            .join("Equalization.md"),
    )?;
    let held = store.active(&Subscriber::Vault)?;
    let mut resolved_files = 0;
    for (_, reference) in wiki_proto::reference::scan(&acme_page) {
        let Ok(Some(hit)) = wiki_proto::resolve(&reference, &held) else {
            continue;
        };
        let target = copy.join("Concepts").join(format!("{}.md", hit.target));
        assert!(
            target.is_file(),
            "`{}` resolved but is not on disk at {}",
            hit.target,
            target.display()
        );
        resolved_files += 1;
    }
    eyre::ensure!(
        resolved_files >= 2,
        "the seeded cross-wiki references should resolve to real files"
    );

    // Refreshing again is a no-op, and the base makes that cheap
    // rather than a re-download.
    let again =
        wiki_live::materialize::refresh_subscription(&upstream, alice.path(), &subscription)?;
    assert_eq!(again.pulled, 0);
    Ok(())
}

/// The `Subscriptions` service, driven the way a client drives it.
///
/// Everything here goes through the trait rather than the store, so
/// what passes is the surface the app actually calls — including the
/// refusals, which are the half most likely to be right in the library
/// and missing over the wire.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_manages_its_own_subscriptions() -> eyre::Result<()> {
    use wiki_proto::service::subscriptions::Subscriptions as _;
    use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

    let tmp = tempfile::tempdir()?;
    let data_root_path = tmp.path();
    // SAFETY: nextest runs one process per test.
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    plant(data_root_path, "acme-audio")?;
    plant(data_root_path, "alice-personal")?;

    let data_root = org_proto::DataRoot::new(data_root_path.to_owned());
    let alice = data_root.org("alice-personal");

    let mut domains = std::collections::HashMap::new();
    domains.insert("acme.test".to_owned(), "acme-audio".to_owned());
    let backend = wiki_live::subscriptions_backend::SubscriptionsBackend::new(
        alice.path().to_path_buf(),
        task_server::core_subscriptions(),
        std::sync::Arc::new(wiki_live::subscriptions_backend::LocalOrgs::new(
            data_root_path.to_path_buf(),
            domains,
        )),
    );
    let me = Subscriber::Vault;

    // Planting handed the vault the core set, and the client can see
    // it without having asked for it.
    let held = backend.list_subscriptions(me.clone())?;
    assert!(
        held.iter().any(|h| h.subscription.core),
        "core arrived without anyone opting in"
    );
    assert_eq!(backend.core_set()?.len(), 1);

    // Take on a wiki.
    let music = Subscription {
        domain: "acme.test".into(),
        slug: "music-theory".into(),
        kind: SourceKind::Wiki,
        title: "Music Theory".into(),
        core: false,
        declined: false,
    };
    backend.subscribe(me.clone(), music.clone())?;
    // Twice is refused rather than silently ignored.
    assert!(backend.subscribe(me.clone(), music.clone()).is_err());

    // Nothing is on disk until it is refreshed.
    let before = backend.list_subscriptions(me.clone())?;
    let entry = before
        .iter()
        .find(|h| h.subscription.slug == "music-theory")
        .expect("held");
    assert_eq!(
        entry.files, 0,
        "a subscription is not a copy until refreshed"
    );

    let report = backend.refresh_subscription(me.clone(), "acme.test/music-theory")?;
    assert!(report.pulled > 0);
    assert!(report.conflicted.is_empty());

    let after = backend.list_subscriptions(me.clone())?;
    let entry = after
        .iter()
        .find(|h| h.subscription.slug == "music-theory")
        .expect("held");
    assert!(entry.files > 0, "the copy is on disk now");

    // Edit the copy, then try to drop it. The refusal is the point:
    // unsubscribing must not discard work upstream has never seen.
    let copy = wiki_live::materialize::local_copy_dir(alice.path(), "acme.test", "music-theory");
    std::fs::write(copy.join("My Notes.md"), "# mine\n")?;
    let refused = backend.unsubscribe(me.clone(), "acme.test/music-theory", false);
    assert!(refused.is_err(), "unsubscribing must ask about local work");
    assert!(
        format!("{}", refused.unwrap_err()).contains("local change"),
        "the refusal should say what is at stake"
    );

    // Forced, it goes.
    backend.unsubscribe(me.clone(), "acme.test/music-theory", true)?;
    assert!(
        !backend
            .list_subscriptions(me.clone())?
            .iter()
            .any(|h| h.subscription.slug == "music-theory")
    );

    // A core one declines rather than disappearing, so it can come
    // back.
    backend.unsubscribe(me.clone(), "fasttrackstudio.app/bible", true)?;
    let bible = backend
        .list_subscriptions(me.clone())?
        .into_iter()
        .find(|h| h.subscription.slug == "bible")
        .expect("a declined core subscription is still listed");
    assert!(bible.subscription.declined);

    // Refreshing something not held is an error, not a silent no-op.
    assert!(backend.refresh_subscription(me, "acme.test/nope").is_err());
    Ok(())
}

/// The Edit lane's seed: each declared wiki carries its declared
/// visibility, the owner holds Editor on Music Theory, and two requests
/// are on the board.
///
/// t[verify wiki.edit.request] — one request is open from a cast member
/// holding no Editor role, carrying the changed page against a named
/// version, and the page it targets is still the committed one.
///
/// t[verify wiki.edit.auto-approve] — the owner's own change is
/// `Accepted` with `auto_approved`, recorded as a request with a
/// tracker row like a reviewed one, and its page carries the change.
///
/// t[verify wiki.edit.tracked] — both requests are issues on the org's
/// board under their own ids, tagged `edit-request`.
#[tokio::test(flavor = "multi_thread")]
async fn demo_plants_the_edit_lane() -> eyre::Result<()> {
    use task::TaskService as _;
    use wiki_proto::service::edits::EditStatus;

    let tmp = tempfile::tempdir()?;
    let data_root = tmp.path();
    unsafe { std::env::set_var("TASK_DEMO_NO_BIBLE", "1") };
    let slug = "acme-audio";
    plant(data_root, slug)?;
    let org = org_proto::DataRoot::new(data_root.to_owned()).org(slug);

    // Visibility, per declaration.
    for declared in task_server::example_org::wikis_of(slug) {
        let wiki_slug = org_proto::wiki_slug(declared.title);
        let config = wiki_live::config::load(&org.named_wiki_dir(&wiki_slug), &wiki_slug)
            .map_err(|e| eyre::eyre!("{wiki_slug}: {e}"))?;
        assert_eq!(
            config.visibility,
            declared.visibility.into(),
            "`{wiki_slug}` planted at the wrong visibility"
        );
    }

    let wiki = task_server::example_org::EDIT_LANE_WIKI;
    let root = org.named_wiki_dir(wiki);
    let config = wiki_live::config::load(&root, wiki).map_err(|e| eyre::eyre!("{e}"))?;
    assert_eq!(config.editors.len(), 1, "the owner alone holds Editor");
    let owner = config.editors[0].clone();

    let requests = wiki_live::edits::list(&root).map_err(|e| eyre::eyre!("{e}"))?;
    let open = requests
        .iter()
        .find(|r| r.title == task_server::example_org::SEED_EDIT_REQUEST_TITLE)
        .expect("the employee's request is planted");
    assert_eq!(open.status, EditStatus::Open);
    assert_ne!(
        open.proposer, owner,
        "the open request is from a non-Editor"
    );
    assert!(!open.auto_approved && !open.held);
    assert_eq!(open.changes.len(), 1);
    assert!(
        !open.changes[0].base_sha256.is_empty(),
        "against a named version"
    );
    let target = std::fs::read_to_string(root.join(&open.changes[0].path))?;
    assert_eq!(
        target, open.changes[0].base_markdown,
        "opening a request must not change the page"
    );

    let auto = requests
        .iter()
        .find(|r| r.title == task_server::example_org::SEED_EDITOR_CHANGE_TITLE)
        .expect("the owner's change is planted");
    assert_eq!(auto.status, EditStatus::Accepted);
    assert!(auto.auto_approved);
    assert_eq!(auto.proposer, owner);
    let landed = std::fs::read_to_string(root.join(&auto.changes[0].path))?;
    assert_eq!(
        landed, auto.changes[0].markdown,
        "the Editor's change did not land"
    );
    let log = std::fs::read_to_string(root.join("log.md"))?;
    assert!(
        log.contains(&auto.id.to_string()),
        "the landing is not logged"
    );

    // Both are issues on the board, under the same ids.
    let tasks = task::TaskBackend::new(org.vault_dir());
    for r in [open, auto] {
        let issue = tasks
            .get(r.id)
            .map_err(|e| eyre::eyre!("issue for {}: {e}", r.title))?;
        assert!(
            issue.tags.0.iter().any(|t| t == "edit-request"),
            "{}: not tagged edit-request ({:?})",
            r.title,
            issue.tags.0
        );
    }

    // A replant adds nothing.
    plant(data_root, slug)?;
    let again = wiki_live::edits::list(&root).map_err(|e| eyre::eyre!("{e}"))?;
    assert_eq!(
        again.len(),
        requests.len(),
        "a replant duplicated edit requests"
    );
    Ok(())
}
