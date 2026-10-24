//! `admin demo` — the example studio, as servers you can sign into.
//!
//! [`crate::example_org`] holds the tree and the cast; this plants them
//! and hands back the endpoint ids. What comes out is the world the
//! integration suite asserts against, on disk, reachable, with known
//! passwords.
//!
//! # One org per data root, because one server hosts one org here
//!
//! `admin seed` puts several orgs in one data root, which is the right
//! shape for eyeballing multi-org UI on one process. This is the other
//! arrangement and the one the product is actually about: **two
//! companies on two machines**, neither a member of the other, sharing
//! one project across a boundary. Federation between two orgs in one
//! process is federation with the interesting part removed — the two
//! failures this repo has actually had were both "it works because both
//! sides share a process".
//!
//! So `--org <slug>` plants exactly one org, and the demo script runs
//! this twice against two data roots and starts two servers.
//!
//! # What it does not do
//!
//! Offer anything or accept anything. Federation — sharing the Shared
//! Project across the org boundary — stays a call a signed-in person
//! makes over the wire; baking it here would bake the interesting half
//! of the demo into a seeder. The org's OWN project directories, by
//! contrast, are adopted as File Roots at plant time: that is the first
//! thing a person would do in the app anyway, and the review surface
//! cannot exist without it (originals never stream — renditions do, and
//! renditions belong to a root).
//!
//! The one thing it does make across an org line is a same-server
//! Sharing Grant (`example_org::SEED_GRANTS`): FastTrackAudio's
//! Crescendum, granted to a person of Rockstars of Tomorrow planted on
//! the same data root. That is the owner's act, not a negotiation
//! between servers, and the lanes that admit the guest need a live grant
//! in the planted world to be reachable from it.
//!
//! DEV ONLY, like `seed`: it plants known-password accounts, so it is
//! compiled out of release builds entirely.

use eyre::{Context, bail};

use crate::example_org::{self, Holds, PASSWORD};

/// `admin demo --org <slug>` — plant one example org on this data root.
///
/// # Errors
///
/// Refuses without an explicit `TASK_DATA_ROOT`, for the reason `seed`
/// refuses: this plants accounts whose passwords are in the source tree.
pub async fn demo(args: &[String]) -> eyre::Result<()> {
    match std::env::var("TASK_DATA_ROOT") {
        Ok(v) if !v.trim().is_empty() => {}
        _ => bail!(
            "refusing to plant a demo: set TASK_DATA_ROOT to a throwaway dir first \
             (this plants known-password accounts — never point it at real data). \
             `just demo` does this for you."
        ),
    }

    let slug = crate::admin_cli::flag(args, "--org").unwrap_or_else(|| "acme-audio".to_owned());
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    plant(&data_root, &slug).await
}

/// Plant the example org `slug` into `data_root` — the body of
/// `admin demo`, callable with an explicit root.
///
/// The app's "Load example studio" (`offline.local`) plants into its
/// own on-device data root with this, so the demo world a person can
/// reach from a local node is the one the servers serve, by the same
/// code. Idempotent, like the command.
///
/// # Errors
///
/// An unknown slug, or any failure planting the tree or the accounts.
pub async fn plant(data_root: &org_proto::DataRoot, slug: &str) -> eyre::Result<()> {
    let slug = slug.to_owned();
    let Some((_, display)) = example_org::ORGS.iter().find(|(s, _)| *s == slug) else {
        bail!(
            "`{slug}` is not in the example. It has: {}",
            example_org::ORGS
                .iter()
                .map(|(s, _)| *s)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };

    data_root
        .ensure()
        .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
    println!("planting {slug} into {}", data_root.path().display());

    // The org is home on its own server. It is the only one here, and
    // "home" is what makes it the identity authority for its own
    // accounts.
    match data_root.init_org(&slug, display, true) {
        Ok(_) => println!("  org: created on disk"),
        Err(e) => println!("  org: already present ({e})"),
    }
    let org = data_root.org(&slug);

    let planted = example_org::install(&org, &slug).wrap_err("plant the example tree")?;
    println!(
        "  tree: {} file(s) written, {} already there",
        planted.written, planted.kept
    );

    #[cfg(feature = "plugin-scripture")]
    plant_bible(&org).await;

    #[cfg(feature = "plugin-wiki")]
    plant_core_subscriptions(&org);

    // Accounts, in this org's own auth store.
    let url = format!("sqlite://{}?mode=rwc", org.auth_db().display());
    let auth = crate::AuthState::open(&url, &crate::auth_secret())
        .await
        .wrap_err_with(|| format!("open auth store for `{slug}`"))?;
    for member in example_org::cast_of(&slug) {
        crate::admin_cli::seed_account(
            &auth,
            member.email,
            PASSWORD,
            member.name,
            None,
            // The owner administers their own company. Nobody else does
            // — Sam and Casey are the two who carry the access story,
            // and an admin role would make both of them meaningless.
            matches!(member.holds, Holds::Owner).then_some("admin"),
            match member.holds {
                Holds::Owner => "owner",
                Holds::Employee => "employee",
                Holds::Client => "client",
            },
        )
        .await?;
    }
    // The bots: accounts of their own under the bot domain, so what
    // they captured or made says so (`attribution`).
    for bot in example_org::bots_of(&slug) {
        crate::admin_cli::seed_account(&auth, bot.email, PASSWORD, bot.name, None, None, "bot")
            .await?;
    }
    // Who captured what, now that there is someone to say.
    match example_org::stamp_capture_creators(&org.vault_dir(), &slug, &auth).await {
        Ok(0) => {}
        Ok(n) => println!("  inbox: {n} capture(s) attributed"),
        Err(e) => println!("  inbox: creators not stamped ({e})"),
    }

    // The Edit lane, now that the accounts it names exist: the owner
    // holds Editor on Music Theory, one request is open from a cast
    // member without the role, and one Editor change went through the
    // lane and was approved within it.
    #[cfg(feature = "plugin-wiki")]
    plant_edit_lane(&org, &slug, &auth).await?;

    // The projects, DECLARED — pages in the org vault, which is what
    // the app's Projects view lists. Idempotent the same way the tree
    // planting is: a page already at the path is left alone.
    let projects = project::ProjectBackend::new(org.projects_dir());
    for declared in example_org::declared_of(&slug) {
        use project::ProjectService as _;
        let info = project::ProjectInfo {
            title: declared.title.to_owned(),
            // The page goes in the project's OWN directory — the one
            // the tree was planted into — rather than at a path derived
            // from the title. The two differ on purpose in this seed:
            // the directory is "First Single - Example Client", because
            // that is what a studio calls a folder, and the project is
            // titled "First Single". Letting `create` pick would put
            // the declaration in a second, empty directory beside the
            // one holding the work.
            path: format!(
                "{}/{}",
                example_org::project_rel(declared.dir),
                org_proto::PROJECT_PAGE
            ),
            capabilities: project::Capabilities::from_names(declared.capabilities.iter().copied()),
            form: declared.form,
            // Only what a person would write. Where the sessions live
            // on disk is the Files page's job to show, not prose.
            details: if declared.clients.is_empty() {
                String::new()
            } else {
                format!("For {}.", declared.clients)
            },
            ..Default::default()
        };
        let made = match projects.create(info) {
            Ok(made) => made,
            Err(project::ProjectError::AlreadyExists(_)) => {
                println!("  project: {} already declared", declared.title);
                continue;
            }
            Err(e) => return Err(eyre::eyre!("declare {}: {e}", declared.title)),
        };
        let mut parts = 0;
        for name in declared.parts {
            projects
                .add_part(made.id, name)
                .map_err(|e| eyre::eyre!("part {name} of {}: {e}", declared.title))?;
            parts += 1;
        }
        match parts {
            0 => println!("  project: {} declared", declared.title),
            n => println!("  project: {} declared with {n} part(s)", declared.title),
        }
    }

    // Deliverables + a believable task board, for every declared
    // project — including ones whose page already existed (a replant
    // is how upgrades reach an old root). Both idempotent by name.
    let tasks_backend = task::TaskBackend::new(org.vault_dir());
    for declared in example_org::declared_of(&slug) {
        use project::ProjectService as _;
        use task::TaskService as _;
        let page = format!(
            "{}/{}",
            example_org::project_rel(declared.dir),
            org_proto::PROJECT_PAGE
        );
        let Some(info) = projects.get_by_path(&page).ok() else {
            continue;
        };
        // Parentage, reconciled on every plant. It is the DECLARED link
        // and nothing else — `project.nesting.explicit`: the fact that
        // `Track Two` sits inside `example-album/` on disk says where
        // its files are and says nothing at all about what it belongs
        // to. A reader that inferred one from the other would be the
        // second, competing definition `project.definition.single`
        // forbids.
        let parent_id = declared.parent.and_then(|parent_dir| {
            let parent_page = format!(
                "{}/{}",
                example_org::project_rel(parent_dir),
                org_proto::PROJECT_PAGE
            );
            projects.get_by_path(&parent_page).ok().map(|p| p.id)
        });
        let info = if parent_id.is_some() && info.parent_id != parent_id {
            let mut np = info.clone();
            np.parent_id = parent_id;
            match projects.update(np) {
                Ok(updated) => {
                    println!("  project: {} parented", declared.title);
                    updated
                }
                Err(e) => return Err(eyre::eyre!("parent {}: {e}", declared.title)),
            }
        } else {
            info
        };
        // Parts, reconciled on every plant: missing ones are added, and
        // the ORDER is enforced to the declaration's — which is the
        // album's playing order, something an earlier seeder (sorted
        // directory names) got wrong on roots planted before this. The
        // ids are untouched, so nothing referencing a part notices.
        if !declared.parts.is_empty() {
            let mut np = info.clone();
            for name in declared.parts {
                if !np.parts.0.iter().any(|x| x.name == *name) {
                    np.parts.0.push(project::Part {
                        id: uuid::Uuid::new_v4(),
                        name: (*name).to_owned(),
                        references: None,
                        components: Vec::new(),
                    });
                }
            }
            let rank = |n: &str| {
                declared
                    .parts
                    .iter()
                    .position(|x| *x == n)
                    .unwrap_or(usize::MAX)
            };
            np.parts.0.sort_by_key(|x| rank(&x.name));
            if np.parts != info.parts {
                projects
                    .update(np)
                    .map_err(|e| eyre::eyre!("order parts of {}: {e}", declared.title))?;
                println!("  parts: {} reconciled to playing order", declared.title);
            }
        }
        for (name, medium, scope, audience) in declared.deliverables {
            let d = project::Deliverable {
                id: uuid::Uuid::new_v4(),
                name: (*name).to_owned(),
                medium: *medium,
                scope: *scope,
                audience: *audience,
            };
            match projects.declare_deliverable(info.id, d) {
                Ok(_) => println!("  deliverable: {} owes \"{name}\"", declared.title),
                Err(project::ProjectError::AlreadyExists(_)) => {}
                Err(e) => return Err(eyre::eyre!("declare \"{name}\": {e}")),
            }
        }
        let existing: Vec<String> = tasks_backend
            .list()
            .map(|ts| ts.into_iter().map(|t| t.title).collect())
            .unwrap_or_default();
        let mut seeded = 0;
        for (title, status, due_days) in declared.tasks {
            if existing.iter().any(|t| t == title) {
                continue;
            }
            let due = due_days.map(|d| {
                (chrono::Utc::now() + chrono::Duration::days(d))
                    .date_naive()
                    .to_string()
            });
            let mut t = task::TaskInfo::new(*title);
            t.status = (*status).to_owned();
            t.due = due;
            t.project_id = Some(info.id);
            // Who made it, per the seed (`attribution`) — and, for
            // Atlas, whom it made it for.
            if let Some(email) = example_org::task_creator(title) {
                if let Some(creator) = example_org::seeded_creator(&auth, email).await {
                    crate::attribution::attribute_task(&mut t, Some(&creator));
                }
            }
            tasks_backend
                .create(t)
                .map_err(|e| eyre::eyre!("seed task \"{title}\": {e}"))?;
            seeded += 1;
        }
        if seeded > 0 {
            println!("  tasks: {seeded} seeded for {}", declared.title);
        }
        // Contexts: where or when a task can be done, so the Relevant
        // filter has something to hold back. Topped up the same way.
        for (title, contexts) in example_org::TASK_CONTEXTS {
            if !declared.tasks.iter().any(|(t, _, _)| t == title) {
                continue;
            }
            let Some(mut t) = tasks_backend
                .list()
                .ok()
                .and_then(|ts| ts.into_iter().find(|t| t.title == *title))
            else {
                continue;
            };
            if !t.contexts.is_empty() {
                continue;
            }
            for c in *contexts {
                t.contexts.push((*c).to_owned());
            }
            tasks_backend
                .update(t)
                .map_err(|e| eyre::eyre!("give \"{title}\" its contexts: {e}"))?;
        }
        // Tags, topped up the same way: a tag already there is kept,
        // one missing is added — `#chore` is how a maintenance hour
        // finds a task with no `@context` at all.
        for (title, tags) in example_org::TASK_TAGS {
            if !declared.tasks.iter().any(|(t, _, _)| t == title) {
                continue;
            }
            let Some(mut t) = tasks_backend
                .list()
                .ok()
                .and_then(|ts| ts.into_iter().find(|t| t.title == *title))
            else {
                continue;
            };
            let missing: Vec<&str> = tags
                .iter()
                .copied()
                .filter(|tag| !t.tags.iter().any(|have| have == tag))
                .collect();
            if missing.is_empty() {
                continue;
            }
            for tag in missing {
                t.tags.0.push(tag.to_owned());
            }
            tasks_backend
                .update(t)
                .map_err(|e| eyre::eyre!("give \"{title}\" its tags: {e}"))?;
        }
        // Descriptions: a body for the document editor to open on.
        // Planted into an empty description only, so an edit made in the
        // demo survives the next plant.
        for (title, details) in example_org::TASK_DETAILS {
            if !declared.tasks.iter().any(|(t, _, _)| t == title) {
                continue;
            }
            let Some(mut t) = tasks_backend
                .list()
                .ok()
                .and_then(|ts| ts.into_iter().find(|t| t.title == *title))
            else {
                continue;
            };
            if !t.details.trim().is_empty() {
                continue;
            }
            t.details = (*details).to_owned();
            tasks_backend
                .update(t)
                .map_err(|e| eyre::eyre!("give \"{title}\" its description: {e}"))?;
        }
        // Owners: a production board shows who is on what. Topped up
        // like everything else — a task that already has an owner keeps
        // it.
        for (title, email) in example_org::TASK_OWNERS {
            if !declared.tasks.iter().any(|(t, _, _)| t == title) {
                continue;
            }
            let Ok(Some(user)) = auth.auth.find_user_by_email(email).await else {
                continue;
            };
            let Some(mut t) = tasks_backend
                .list()
                .ok()
                .and_then(|ts| ts.into_iter().find(|t| t.title == *title))
            else {
                continue;
            };
            let wf = t.workflow.get_or_insert_with(Default::default);
            if !wf.assignees.0.is_empty() {
                continue;
            }
            wf.assignees.0 = vec![task::workflows_proto::AgentRef::Human {
                user_id: user.id.to_string(),
                display_name: user.name,
            }];
            tasks_backend
                .update(t)
                .map_err(|e| eyre::eyre!("give \"{title}\" its owner: {e}"))?;
        }
    }

    // History, now that the cast and the board exist: the board's recent
    // past and a comment thread, so `/activity` and a task's page open
    // on something. Idempotent (fixed ids; comments matched by body).
    match example_org::plant_activity(&org, &slug, &auth).await {
        Ok(0) => {}
        Ok(n) => println!("  activity: {n} entries planted"),
        Err(e) => println!("  activity: not planted ({e})"),
    }

    // Money, where the seed declares some: the billable clients project
    // pages point at, a member's rate on a project, billed time and the
    // invoice raised from it. FastTrackAudio's, so a Sharing Grant on
    // Crescendum has real billing beside it to keep out
    // (`sharing.grant.exclusions`).
    plant_billing(&org, &slug, &auth)
        .await
        .wrap_err("plant the billing")?;

    // Video deliverables: generated rather than committed — even a tiny
    // mp4 is binary weight git keeps forever, and ffmpeg synthesises a
    // perfectly good one in a second (the dev shell carries ffmpeg).
    // They land INSIDE the project's own directory (`Deliverables/`),
    // not in `resources/`: video plays in the review surface, which
    // streams *renditions* from a File Root — so the original has to
    // live where the root (adopted below) can hold it. Machines without
    // ffmpeg get the audio deliverables (committed WAVs) and a note;
    // the items stay honestly outstanding.
    //
    // The ROUGH CUT is what lands here — the final is rendered after
    // adoption, with a checkpoint on each side, so every fresh video
    // arrives with a real two-version history and the review screen's
    // version switcher and compare have something true to show.
    let mut fresh_videos: Vec<(&'static str, std::path::PathBuf, &'static str)> = Vec::new();
    for declared in example_org::declared_of(&slug) {
        for (name, medium, _, _) in declared.deliverables {
            if *medium != project::Medium::Video {
                continue;
            }
            let dest = example_org::project_path(&org, declared.dir)
                .join("Deliverables")
                .join(format!("{name}.mp4"));
            if dest.exists() {
                println!("  video: \"{name}\" already generated");
                continue;
            }
            if !which_ffmpeg() {
                println!("  video: no ffmpeg on PATH — \"{name}\" stays outstanding");
                continue;
            }
            std::fs::create_dir_all(dest.parent().expect("parent"))?;
            if synth_video(&dest, VideoCut::Rough) {
                println!("  video: \"{name}\" rough cut for {}", declared.title);
                fresh_videos.push((declared.title, dest, name));
            } else {
                println!("  video: ffmpeg failed — \"{name}\" stays outstanding");
            }
        }
    }

    // Audio masters live twice, deliberately: as *songs* under
    // `resources/` (the global player's colocated-song convention) and
    // as the delivered file in the project's `Deliverables/` — which is
    // what the review surface (waveform stage, timecoded comments)
    // mounts on. Same bytes, two conventions, one copy at plant time.
    for declared in example_org::declared_of(&slug) {
        for (name, medium, scope, _) in declared.deliverables {
            if *medium != project::Medium::Audio || *scope != project::Scope::WholeProject {
                continue;
            }
            let src = org
                .resources_dir()
                .join("songs")
                .join(example_org::song_slug(name))
                .join(format!("{name}.wav"));
            let dest = example_org::project_path(&org, declared.dir)
                .join("Deliverables")
                .join(format!("{name}.wav"));
            if dest.exists() || !src.is_file() {
                continue;
            }
            std::fs::create_dir_all(dest.parent().expect("parent"))?;
            std::fs::copy(&src, &dest)?;
            println!("  audio: \"{name}\" delivered into {}", declared.title);
        }
    }

    // Each declared project's directory becomes a File Root named after
    // the project — what a person would do first in the app, seeded so
    // the review surface works out of the box (its player streams
    // renditions, and renditions belong to a root). Idempotent by name;
    // `settled` waits out the catalogue walk so the videos generated
    // above are browsable the moment the server boots.
    {
        use files::service::roots::RootsService;
        let backend = files::FilesBackend::new(org.path().join("files"), org.vault_dir())
            .map_err(|e| eyre::eyre!("files backend: {e}"))?;
        let existing = RootsService::list(&backend)
            .await
            .map_err(|e| eyre::eyre!("list roots: {e}"))?;
        for declared in example_org::declared_of(&slug) {
            if existing.iter().any(|r| r.name == declared.title) {
                continue;
            }
            let dir = example_org::project_path(&org, declared.dir);
            if !dir.is_dir() {
                continue;
            }
            // `adopt_org_tree`, not the wire verb: a project's directory
            // is on the Projects tier, which sits outside `<org>/files/`
            // like the vault and the wiki do, and `RootsService::adopt`
            // confines to that boundary on purpose. This registers
            // without the confinement and drives the same catalogue walk
            // — both halves, because the review surface streams
            // renditions and a rendition belongs to a catalogued file.
            let root = backend
                .adopt_org_tree(&dir, declared.title, true)
                .map_err(|e| eyre::eyre!("adopt {}: {e}", declared.title))?;
            backend.settled(root).await;
            println!("  root: {} adopted in place", declared.title);
        }

        // The final cut, as HISTORY: checkpoint the rough cut the
        // catalogue just took in, render the final over it, checkpoint
        // again. Every fresh video arrives with two honest versions —
        // which is what makes the review screen's version switcher and
        // side-by-side compare demoable rather than decorative.
        if !fresh_videos.is_empty() {
            let roots = RootsService::list(&backend)
                .await
                .map_err(|e| eyre::eyre!("list roots: {e}"))?;
            for (title, dest, name) in &fresh_videos {
                let Some(root) = roots.iter().find(|r| r.name == *title) else {
                    continue;
                };
                if let Err(e) = files::service::VersionService::checkpoint(
                    &backend,
                    files::RootId::new(root.id),
                    Some(format!("{name} — rough cut")),
                )
                .await
                {
                    println!("  video: couldn't checkpoint rough cut of \"{name}\" ({e})");
                    continue;
                }
                if !synth_video(dest, VideoCut::Final) {
                    println!("  video: final render of \"{name}\" failed — rough cut stands");
                    continue;
                }
                match files::service::VersionService::checkpoint(
                    &backend,
                    files::RootId::new(root.id),
                    Some(format!("{name} — final")),
                )
                .await
                {
                    Ok(_) => println!("  video: \"{name}\" finalled — two versions on record"),
                    Err(e) => println!("  video: final of \"{name}\" not checkpointed ({e})"),
                }
            }
        }

        if slug == example_org::BOUND_SAMPLE.org {
            match plant_bound_sample(&backend, &org).await {
                Ok(line) => println!("  sample: {line}"),
                Err(e) => println!("  sample: the bound sample was not planted ({e})"),
            }
        }

        plant_devices(&backend, &slug).await?;

        // Last, because a grant's preview digest is over the subtree as
        // it now stands — File Roots included, and they were adopted
        // just above.
        plant_grants(data_root, &org, &slug, &auth, &backend)
            .await
            .wrap_err("plant the sharing grants")?;
    }

    println!("\nplanted. sign in with any of:");
    for member in example_org::cast_of(&slug) {
        println!(
            "  {:<20} {PASSWORD}   ({:?}{})",
            member.email,
            member.holds,
            if member.scope.is_empty() {
                String::new()
            } else {
                format!(" of {}", member.scope)
            }
        );
    }
    println!("\nprojects on disk, adopted as File Roots:");
    let projects = org.projects_dir();
    if let Ok(entries) = std::fs::read_dir(&projects) {
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            println!("  {}", projects.join(name).display());
        }
    }
    Ok(())
}

/// Plant the org's declared money: the `timer_clients` rows its project
/// pages point at (`DECLARED_CLIENTS`), and for each `SEED_BILLING`
/// entry the member's rate on the project, one billable session and the
/// invoice generated from it.
///
/// Idempotent by what already exists: a client row is kept by id, the
/// rate is an upsert, a session is logged only when the member has none
/// on the project, and an invoice is generated only while the project
/// still has billable time on no invoice — which is false the moment the
/// first plant raised one.
async fn plant_billing(
    org: &org_proto::OrgRoot,
    slug: &str,
    auth: &crate::AuthState,
) -> eyre::Result<()> {
    use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _, Set};
    use sea_orm_migration::MigratorTrait as _;
    use timer::entity::{ClientActive, ClientEntity, WorkSessionColumn, WorkSessionEntity};
    use timer_proto::TimerService as _;

    if example_org::clients_of(slug).next().is_none()
        && example_org::billing_of(slug).next().is_none()
    {
        return Ok(());
    }
    let timer_url = format!("sqlite://{}?mode=rwc", org.timer_db().display());
    let timer_conn = sea_orm::Database::connect(&timer_url)
        .await
        .wrap_err("open the timer store")?;
    timer::Migrator::up(&timer_conn, None)
        .await
        .wrap_err("migrate the timer store")?;

    for c in example_org::clients_of(slug) {
        if ClientEntity::find_by_id(c.id)
            .one(&timer_conn)
            .await
            .wrap_err("look up a client")?
            .is_some()
        {
            continue;
        }
        let now = chrono::Utc::now();
        ClientEntity::insert(ClientActive {
            id: Set(c.id),
            org_id: Set(uuid::Uuid::nil()),
            name: Set(c.name.to_owned()),
            notes: Set(String::new()),
            archived: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .exec(&timer_conn)
        .await
        .wrap_err_with(|| format!("add the client {}", c.name))?;
        println!("  client: {} on the books", c.name);
    }

    let timer = timer::Store::new(
        timer_conn.clone(),
        std::sync::Arc::new(timer::store::VaultProjectDefaults {
            projects_root: org.projects_dir(),
        }),
    );
    let projects = project::ProjectBackend::new(org.projects_dir());
    for b in example_org::billing_of(slug) {
        use project::ProjectService as _;
        let Some(declared) = example_org::declared_of(slug).find(|p| p.title == b.project) else {
            bail!(
                "billing names `{}`, which {slug} does not declare",
                b.project
            );
        };
        let page = format!(
            "{}/{}",
            example_org::project_rel(declared.dir),
            org_proto::PROJECT_PAGE
        );
        let info = projects
            .get_by_path(&page)
            .map_err(|e| eyre::eyre!("billing: no page for {}: {e}", b.project))?;
        let Some(member) = auth
            .auth
            .find_user_by_email(b.member)
            .await
            .map_err(|e| eyre::eyre!("look up `{}`: {e:?}", b.member))?
        else {
            println!(
                "  billing: no account `{}` — {} not billed",
                b.member, b.project
            );
            continue;
        };
        timer
            .set_project_member_rate(info.id, member.id, b.hourly_cents)
            .await
            .map_err(|e| eyre::eyre!("rate on {}: {e:?}", b.project))?;
        // The declared session itself, not any time on the project: time a
        // demo user logged there must not stand in for the seed's.
        let description = format!("{} — first batch", b.project);
        let logged = WorkSessionEntity::find()
            .filter(WorkSessionColumn::ProjectId.eq(info.id))
            .filter(WorkSessionColumn::UserId.eq(member.id))
            .filter(WorkSessionColumn::Description.eq(description.clone()))
            .one(&timer_conn)
            .await
            .wrap_err("look up billed time")?
            .is_some();
        if !logged {
            let end = chrono::Utc::now() - chrono::Duration::days(2);
            timer
                .log_session(timer_proto::LogSessionRequest {
                    user_id: member.id,
                    org_id: uuid::Uuid::nil(),
                    project_id: Some(info.id),
                    project_path: info.path.clone(),
                    task_note_path: String::new(),
                    description,
                    start_time: end - chrono::Duration::hours(b.hours),
                    end_time: end,
                    billable_override: Some(true),
                })
                .await
                .map_err(|e| eyre::eyre!("log time on {}: {e:?}", b.project))?;
            println!("  timer: {}h billed on {}", b.hours, b.project);
        }
        #[cfg(feature = "plugin-finance")]
        plant_invoice(org, &timer_conn, info.id, b).await?;
    }
    Ok(())
}

/// Raise the invoice for a project's billable time not yet on one.
#[cfg(feature = "plugin-finance")]
async fn plant_invoice(
    org: &org_proto::OrgRoot,
    timer_conn: &sea_orm::DatabaseConnection,
    project_id: uuid::Uuid,
    b: &'static example_org::DeclaredBilling,
) -> eyre::Result<()> {
    use finance_proto::service::invoicing::{GenerateInvoice, Invoicing as _};
    use sea_orm_migration::MigratorTrait as _;

    let finance_url = format!("sqlite://{}?mode=rwc", org.finance_db().display());
    let finance_conn = sea_orm::Database::connect(&finance_url)
        .await
        .wrap_err("open the finance store")?;
    finance_db::Migrator::up(&finance_conn, None)
        .await
        .wrap_err("migrate the finance store")?;
    let ledger = finance::LedgerService::new(finance_conn.clone())
        .map_err(|e| eyre::eyre!("ledger: {e}"))?;
    let book = org
        .manifest()
        .map_or_else(|_| "Business".into(), |m| m.display_name);
    let invoicing = finance::FinanceBackend::new(finance_conn, timer_conn.clone(), book, ledger)
        .map_err(|e| eyre::eyre!("finance backend: {e}"))?;
    // The backend drives its async store with `block_on`, which only a
    // blocking thread may do.
    let raised = tokio::task::spawn_blocking(move || -> Result<
        Option<finance_proto::invoice::Invoice>,
        finance_proto::error::FinanceError,
    > {
        let open = invoicing
            .uninvoiced()?
            .into_iter()
            .any(|g| g.project_id == Some(project_id) && g.session_count > 0);
        if !open {
            return Ok(None);
        }
        invoicing
            .generate_invoice(GenerateInvoice {
                project_id: Some(project_id),
                tag: String::new(),
                client_name: b.client.to_owned(),
                since: String::new(),
                until: String::new(),
                net_days: 30,
            })
            .map(Some)
    })
    .await
    .wrap_err("the invoicing thread")?
    .map_err(|e| eyre::eyre!("invoice {}: {e:?}", b.project))?;
    if let Some(invoice) = raised {
        println!(
            "  invoice: {} to {} for {} ({} {})",
            invoice.id, b.client, b.project, invoice.currency, invoice.total_minor
        );
    }
    Ok(())
}

/// Make the org's declared Sharing Grants (`SEED_GRANTS`,
/// `docs/spec/sharing.md`): one person of another org on this data root,
/// read + comment on one project's subtree, with no membership here.
///
/// The row is written the way the `sharing` service writes one — the
/// owner as `granted_by`, the preview digest over the subtree as it now
/// stands, a `grant.created` audit line — but directly, because a seed
/// has no signed-in owner to call the service as. Grants are immutable
/// and never deleted, so idempotency is "a live grant for this person on
/// this root already exists": then nothing is written.
///
/// The guest's account lives in the guest org's own store, so that org
/// must be planted on this data root first (`just demo` plants
/// Rockstars of Tomorrow before FastTrackAudio). Without it the grant is
/// skipped with a line saying so, never invented.
async fn plant_grants(
    data_root: &org_proto::DataRoot,
    org: &org_proto::OrgRoot,
    slug: &str,
    auth: &crate::AuthState,
    files: &files::FilesBackend,
) -> eyre::Result<()> {
    use crate::sharing::{self, GrantEvent, GrantEventKind, SharingGrant, Subtree as _};
    use project::ProjectService as _;

    if example_org::grants_of(slug).next().is_none() {
        return Ok(());
    }
    let store = sharing::GrantStore::open(&org.sharing_db(), slug)
        .await
        .wrap_err("open the sharing store")?;
    let projects = project::ProjectBackend::new(org.projects_dir());
    for g in example_org::grants_of(slug) {
        let guest_root = data_root.org(g.guest_org);
        if !guest_root.auth_db().exists() {
            println!(
                "  grant: {} not planted — `{}` is not on this data root (plant it first)",
                g.root, g.guest_org
            );
            continue;
        }
        let guest_auth = crate::AuthState::open(
            &format!("sqlite://{}?mode=rwc", guest_root.auth_db().display()),
            &crate::auth_secret(),
        )
        .await
        .wrap_err_with(|| format!("open the auth store of `{}`", g.guest_org))?;
        let Some(guest) = guest_auth
            .auth
            .find_user_by_email(g.subject)
            .await
            .map_err(|e| eyre::eyre!("look up `{}`: {e:?}", g.subject))?
        else {
            println!(
                "  grant: {} not planted — no `{}` in `{}`",
                g.root, g.subject, g.guest_org
            );
            continue;
        };
        let Some(owner) = auth
            .auth
            .find_user_by_email(g.granted_by)
            .await
            .map_err(|e| eyre::eyre!("look up `{}`: {e:?}", g.granted_by))?
        else {
            bail!(
                "the grant on {} names `{}`, who is not in {slug}",
                g.root,
                g.granted_by
            );
        };
        let page = format!(
            "{}/{}",
            example_org::project_rel(g.root),
            org_proto::PROJECT_PAGE
        );
        let root = projects
            .get_by_path(&page)
            .map_err(|e| eyre::eyre!("grant root {}: {e}", g.root))?
            .id;
        let now = sharing::now_unix();
        let held = store
            .for_subject(guest.id, root)
            .await
            .wrap_err("read the subject's grants")?;
        if held.iter().any(|x| sharing::is_live(x, now)) {
            println!("  grant: {} already shared with {}", g.root, g.subject);
            continue;
        }
        // The same adapter the server builds (`build_org_state`), over
        // the same stores, so the digest is the one a preview shows.
        let subtree = sharing::OwnerStore::new(projects.clone(), org.projects_dir())
            .with_tasks(task::TaskBackend::new(org.vault_dir()))
            .with_milestones(milestone::MilestoneBackend::new(org.projects_dir()))
            .with_workstreams(workstream::WorkstreamBackend::new(org.projects_dir()))
            .with_files(files.clone());
        let visible = tokio::task::spawn_blocking(move || subtree.visible(root))
            .await
            .wrap_err("the subtree walk")?
            .ok_or_else(|| eyre::eyre!("the subtree of {} could not be read", g.root))?;
        let grant = SharingGrant {
            id: uuid::Uuid::new_v4(),
            owner_org: slug.to_owned(),
            root_id: root,
            subject: guest.id,
            guest_org: g.guest_org.to_owned(),
            capabilities: g.capabilities,
            expires_at: None,
            granted_by: owner.id,
            granted_at: now,
            revoked_at: None,
            revoked_by: None,
            preview_digest: sharing::digest(root, &visible),
        };
        store.insert(&grant).await.wrap_err("write the grant")?;
        sharing::audit(
            slug,
            &GrantEvent::of(&grant, GrantEventKind::Created, now, Some(owner.id)),
        );
        println!(
            "  grant: {} shared with {} of {} ({} item(s), read + comment)",
            g.root,
            g.subject,
            g.guest_org,
            visible.len()
        );
    }
    Ok(())
}

/// Pair the org's declared machines and record what each keeps
/// (`files.device.identity`, `files.sync.selective`).
///
/// Enrolled the way the app enrols a machine — by the endpoint id it
/// holds — and each subscription recorded under that machine's own
/// device row, so the devices list shows two machines that took
/// different halves of one album rather than one shared answer.
///
/// Idempotent by omission, like the tree: a machine already enrolled is
/// left exactly as it is. Re-enrolling would un-revoke it, and a demo
/// user who revoked the producer's laptop must not find it back after a
/// replant.
async fn plant_devices(backend: &files::FilesBackend, slug: &str) -> eyre::Result<()> {
    use files::service::roots::RootsService;
    use files::service::sync::{FacetName, SyncService};

    let declared: Vec<_> = example_org::devices_of(slug).collect();
    if declared.is_empty() {
        return Ok(());
    }
    let roots = RootsService::list(backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    let known = SyncService::devices(backend)
        .await
        .map_err(|e| eyre::eyre!("list devices: {e}"))?;
    for device in declared {
        let endpoint = device.endpoint();
        if known
            .iter()
            .any(|d| d.endpoint.as_deref() == Some(endpoint.as_str()))
        {
            println!("  device: {} already paired", device.name);
            continue;
        }
        let Some(root) = roots.iter().find(|r| r.name == device.project) else {
            println!(
                "  device: {} not paired — no root named {}",
                device.name, device.project
            );
            continue;
        };
        let row = backend
            .enroll_device(endpoint, device.name.to_owned())
            .await
            .map_err(|e| eyre::eyre!("enrol {}: {e}", device.name))?;
        backend
            .record_device_subscription(
                row.id,
                files::RootId::new(root.id),
                device
                    .facets
                    .iter()
                    .map(|f| FacetName((*f).to_owned()))
                    .collect(),
            )
            .await
            .map_err(|e| eyre::eyre!("subscribe {}: {e}", device.name))?;
        println!(
            "  device: {} paired, keeps {} of {}",
            device.name,
            device.facets.join(", "),
            device.project
        );
    }
    Ok(())
}

/// Task as an app's store, planted: make a root the way an app makes one,
/// save a file into it through the upload lane, and pin a sample manifest
/// to exactly those bytes.
///
/// Through `files_client` over an in-process router rather than through
/// the backend's own methods, so the seed reaches the lanes the way
/// Signal would — and a lane that stopped working for apps stops the
/// seed too.
///
/// Idempotent: the root is found again by its directory, a file already
/// at the path is not saved twice (create-only), and the manifest is
/// re-pinned to whatever is there now.
async fn plant_bound_sample(
    backend: &files::FilesBackend,
    org: &org_proto::OrgRoot,
) -> eyre::Result<String> {
    use architect::{LayerRouter, LocalServer, Scope};
    use files_client::{FilesClient, Save, root_id};
    use resources_proto::ResourcesService as _;

    let bound = example_org::BOUND_SAMPLE;
    let audio = example_org::studio_file(bound.source)
        .ok_or_else(|| eyre::eyre!("{} is not committed", bound.source))?;

    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::tree_layer(backend.clone()))
            .merge(files_proto::upload_layer(backend.clone()))
            .merge(files_proto::media_layer(backend.clone()))
            .merge(files_proto::media_stream_layer(backend.clone())),
        Scope::new(),
    );
    let dialled = |e| eyre::eyre!("in-process dial: {e:?}");
    let client = FilesClient::new(
        local.establish().await.map_err(dialled)?,
        local.establish().await.map_err(dialled)?,
        local.establish().await.map_err(dialled)?,
        local.establish().await.map_err(dialled)?,
        local.establish().await.map_err(dialled)?,
    );

    let store = client
        .ensure_root(
            bound.root_dir,
            bound.root_name,
            files_proto::model::RootFlavor::Media,
        )
        .await
        .map_err(|e| eyre::eyre!("ensure root: {e}"))?;
    let root = root_id(&store);
    let entry = match client
        .put(root, bound.path, audio, Save::create_only())
        .await
    {
        Ok(entry) => entry,
        // Already planted. Pin whatever is there.
        Err(e) if e.is_stale() => client
            .entry(root, bound.path)
            .await
            .map_err(|e| eyre::eyre!("read back: {e}"))?,
        Err(e) => return Err(eyre::eyre!("save: {e}")),
    };
    let pin = entry.content.map(|c| c.0).unwrap_or_default();

    let resources = resources::ResourcesBackend::new(org.resources_dir());
    let mut sample = resources
        .sample(bound.slug)
        .map_err(|e| eyre::eyre!("read manifest: {e:?}"))?;
    sample.content = resources_proto::ContentRef {
        root_id: root.to_string(),
        path: bound.path.to_string(),
        content: pin.clone(),
    };
    resources
        .upsert_sample(sample)
        .map_err(|e| eyre::eyre!("bind manifest: {e:?}"))?;
    Ok(format!(
        "sample:{} bound to {}/{} in \"{}\", pinned {}",
        bound.slug,
        bound.root_dir,
        bound.path,
        bound.root_name,
        pin.get(..12).unwrap_or(&pin)
    ))
}

/// Which pass of a deliverable synth this is — the two must LOOK
/// different, or the review screen's compare demonstrates nothing.
#[derive(Clone, Copy)]
enum VideoCut {
    /// Classic bars, shorter, a low tone: obviously the draft.
    Rough,
    /// The moving testsrc2 pattern, full length, brighter tone.
    Final,
}

/// Synthesize a small deliverable mp4 at `dest` (overwrites — the
/// final pass renders over the rough cut on purpose; the version store
/// holds the history).
///
/// Slow animated gradients, not test patterns: every surface that
/// shows a frame of this file — home card art, the review stage, the
/// filmstrip scrub — inherits its look, so the synth has to read as a
/// deliverable, not as a broadcast calibration chart. The rough cut is
/// desaturated and short; the final runs the product's indigo.
fn synth_video(dest: &std::path::Path, cut: VideoCut) -> bool {
    let (video, audio) = match cut {
        VideoCut::Rough => (
            "gradients=s=640x360:c0=0x27272a:c1=0x3f3f46:c2=0x18181b:n=3:speed=0.02:duration=6:rate=24",
            "sine=frequency=220:duration=6",
        ),
        VideoCut::Final => (
            "gradients=s=640x360:c0=0x1e1b4b:c1=0x312e81:c2=0x0f172a:c3=0x6366f1:n=4:speed=0.015:duration=8:rate=24",
            "sine=frequency=330:duration=8",
        ),
    };
    std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            video,
            "-f",
            "lavfi",
            "-i",
            audio,
            "-filter_complex",
            "[1:a]volume=0.35[a]",
            "-map",
            "0:v",
            "-map",
            "[a]",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-c:a",
            "aac",
            "-b:a",
            "48k",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            // One encoder thread: a clip this small encodes in well under
            // a second on one core, and libx264's default (a thread per
            // core) let a dozen concurrent plants (the test suite) run
            // ~50 encoders over 16 cores and time each other out.
            "-threads",
            "1",
        ])
        .arg(dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is an `ffmpeg` on PATH? A probe, not a version check — the seeder's
/// synth args have worked on every ffmpeg this decade.
fn which_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Put a Bible in the org's resource library, so the seeded Bible
/// Study wiki has something to anchor to and the scripture reader has
/// something to read.
///
/// Public domain only — `scripture::pull` refuses anything else, and
/// the demo asks for WEB by name. A licensed edition could not be
/// planted even if someone tried.
///
/// **Never fails the plant.** A demo that will not stand up because a
/// download timed out is worse than a demo with an empty reader, and
/// the reader already handles an absent corpus (`load_resource_root`
/// returns an empty store for a missing directory). So every failure
/// here is a printed line and nothing more.
///
/// Offline is a first-class path: the archive is cached under
/// `$TASK_BIBLE_CACHE`, so the first plant on a machine downloads and
/// every plant after that — including on a plane — installs from disk.
/// `TASK_DEMO_NO_BIBLE=1` skips it entirely.
///
/// One copy per org for now, which `wiki.core.default` will replace:
/// core membership is meant to be a property of the source rather than
/// a copy of it, so once subscription exists these orgs will share one
/// corpus instead of each holding a canon.
#[cfg(feature = "plugin-scripture")]
async fn plant_bible(org: &org_proto::OrgRoot) {
    const TRANSLATION: &str = "WEB";

    if std::env::var_os("TASK_DEMO_NO_BIBLE").is_some() {
        println!("  bible: skipped (TASK_DEMO_NO_BIBLE)");
        return;
    }
    let dest = org.bible_dir(TRANSLATION);
    if dest.is_dir()
        && std::fs::read_dir(&dest)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        println!("  bible: {TRANSLATION} already installed");
        return;
    }
    match scripture::pull(TRANSLATION, &dest).await {
        Ok(pulled) => println!(
            "  bible: {} books of {} ({})",
            pulled.books.len(),
            pulled.id,
            if pulled.from_cache {
                "from cache"
            } else {
                "downloaded"
            }
        ),
        Err(e) => println!(
            "  bible: not installed ({e}).\n         \
             The scripture reader will be empty until you run\n         \
             `task-server admin bible install --org {}`.",
            org.slug()
        ),
    }
    plant_study(org).await;
}

/// The Bible Study wiki's study data: two more translations (so a verse
/// card has something to switch to and compare), the Strong's lexicon
/// and the Hebrew and Greek texts (so a word page's study has an entry
/// and every occurrence). Only for an org whose seed has the Bible Study
/// Library — the others would carry fifty megabytes nobody reads. Never
/// fails the plant, and installs from the download cache after the first
/// time.
#[cfg(feature = "plugin-scripture")]
async fn plant_study(org: &org_proto::OrgRoot) {
    if !org
        .path()
        .join("wikis")
        .join("bible-study-library")
        .is_dir()
    {
        return;
    }
    let resources = org.resources_dir();
    let filled = |dir: &std::path::Path| {
        std::fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    };
    for tx in ["KJV", "BSB"] {
        let dest = org.bible_dir(tx);
        if filled(&dest) {
            continue;
        }
        match scripture::pull(tx, &dest).await {
            Ok(p) => println!("  bible: {} books of {}", p.books.len(), p.id),
            Err(e) => println!("  bible: {tx} not installed ({e})"),
        }
    }
    let lexicon = resources.join("lexicon").join("strongs");
    if !filled(&lexicon) {
        match scripture::study_pull::pull_lexicon(&lexicon).await {
            Ok((g, h)) => println!("  lexicon: {g} Greek, {h} Hebrew entries"),
            Err(e) => println!(
                "  lexicon: not installed ({e}) — `task-server admin bible lexicon --org {}`",
                org.slug()
            ),
        }
    }
    for id in scripture::study_pull::ORIGINALS {
        let dest = resources.join("original").join(id);
        if filled(&dest) {
            continue;
        }
        match scripture::study_pull::pull_original(id, &dest).await {
            Ok(n) => println!("  original: {id}, {n} verses"),
            Err(e) => println!(
                "  original: {id} not installed ({e}) — `task-server admin bible original --org {}`",
                org.slug()
            ),
        }
    }
}

/// Hand the freshly planted org's vault and wikis the core set.
///
/// `wiki.core.default` says a vault carries the core set *from the
/// moment it exists* — so planting one has to do this, not only
/// booting a server against it. The boot-time sweep in
/// `crate::OrgServices` is the retroactive half (`wiki.core.retroactive`),
/// for orgs that existed before a source became core; this is the half
/// that makes a brand-new vault able to reference a verse in its first
/// line without anything else having run.
///
/// Never fatal, for the reason the Bible install is not: an org that
/// cannot record its subscriptions should still plant.
/// A caller pinned to one account: the seeder acting *as* a cast
/// member, so what it plants is what that person would have made.
#[cfg(feature = "plugin-wiki")]
struct As(String);

#[cfg(feature = "plugin-wiki")]
impl wiki_live::backend::Caller for As {
    fn principal(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// Plant the Edit lane's seed on [`example_org::EDIT_LANE_WIKI`]:
/// the owner as Editor, one open request from the employee
/// (`wiki.edit.request`), one auto-approved Editor change
/// (`wiki.edit.auto-approve`). Each is idempotent — the grant is a
/// set-add, the requests are matched by title.
///
/// Through the real backend rather than by writing files: the request
/// then has its tracker row on the org's board (`wiki.edit.tracked`)
/// and the Editor's change went through the same lane a person's does.
#[cfg(feature = "plugin-wiki")]
async fn plant_edit_lane(
    org: &org_proto::OrgRoot,
    slug: &str,
    auth: &crate::AuthState,
) -> eyre::Result<()> {
    use std::sync::Arc;

    use wiki_proto::service::Pages as _;
    use wiki_proto::service::edits::{Edits as _, NewEditRequest, PageChange};

    let wiki = example_org::EDIT_LANE_WIKI;
    let root = org.named_wiki_dir(wiki);
    if !root.is_dir() {
        return Ok(());
    }
    let cast = example_org::cast_of(slug);
    let (Some(owner), Some(employee)) = (
        cast.iter().find(|m| matches!(m.holds, Holds::Owner)),
        cast.iter().find(|m| matches!(m.holds, Holds::Employee)),
    ) else {
        return Ok(());
    };
    async fn id_of(auth: &crate::AuthState, email: &str) -> eyre::Result<String> {
        auth.auth
            .find_user_by_email(email)
            .await
            .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
            .map(|u| u.id.to_string())
            .ok_or_else(|| eyre::eyre!("`{email}` was not created"))
    }
    let owner_id = id_of(auth, owner.email).await?;
    let employee_id = id_of(auth, employee.email).await?;

    // The owner is the first Editor — which is what turns the lane on
    // for this wiki (`wiki.edit.editor`).
    let config = wiki_live::config::update(&root, wiki, |c| {
        if !c.is_editor(&owner_id) {
            c.editors.push(owner_id.clone());
        }
    })
    .map_err(|e| eyre::eyre!("declare {}'s Editor: {e}", owner.name))?;
    println!(
        "  editors: {} holds Editor on {wiki} ({} total)",
        owner.name,
        config.editors.len()
    );

    let roots: std::collections::HashMap<String, std::path::PathBuf> =
        org.named_wikis().into_iter().collect();
    let tracker = Arc::new(crate::wiki_tracker::TaskTracker::new(
        task::TaskBackend::new(org.vault_dir()),
    ));
    let lane_as = |who: &str| {
        wiki_live::edits_backend::EditsBackend::new(
            wiki_live::WikiBackend::with_roots_under(roots.clone(), org.wikis_dir())
                .with_caller(Arc::new(As(who.to_owned()))),
            tracker.clone(),
        )
    };
    let existing: Vec<String> = wiki_live::edits::list(&root)
        .map_err(|e| eyre::eyre!("list edit requests: {e}"))?
        .into_iter()
        .map(|r| r.title)
        .collect();

    if !existing
        .iter()
        .any(|t| t == example_org::SEED_EDIT_REQUEST_TITLE)
    {
        let sam = lane_as(&employee_id);
        let path = "Concepts/Ionian.md";
        let page = sam
            .wiki()
            .read_page(wiki, path)
            .map_err(|e| eyre::eyre!("read {path}: {e}"))?;
        let proposed = page.markdown.replacen(
            "## See also",
            "## Hearing it\n\nPlay C major over a C drone and stop on B: the pull back to C is \
             the leading tone doing its work.\n\n## See also",
            1,
        );
        sam.open_edit_request(
            wiki,
            NewEditRequest {
                title: example_org::SEED_EDIT_REQUEST_TITLE.to_owned(),
                summary: "A way to hear the major 7th, for players who learn by ear.".into(),
                changes: vec![PageChange {
                    path: path.to_owned(),
                    base_sha256: page.sha256,
                    base_markdown: page.markdown,
                    markdown: proposed,
                    delete: false,
                }],
                request_review: false,
            },
        )
        .map_err(|e| eyre::eyre!("open {}'s edit request: {e}", employee.name))?;
        println!(
            "  edit request: \"{}\" opened by {}",
            example_org::SEED_EDIT_REQUEST_TITLE,
            employee.name
        );
    }

    if !existing
        .iter()
        .any(|t| t == example_org::SEED_EDITOR_CHANGE_TITLE)
    {
        let alice = lane_as(&owner_id);
        let path = "Concepts/Modes.md";
        let page = alice
            .wiki()
            .read_page(wiki, path)
            .map_err(|e| eyre::eyre!("read {path}: {e}"))?;
        let proposed = format!(
            "{}\n## Naming\n\nThe Greek names are medieval labels, not the Greeks' own — the \
             ancient *tonoi* were tuned differently and the names were reassigned later.\n",
            page.markdown.trim_end_matches('\n')
        );
        let made = alice
            .open_edit_request(
                wiki,
                NewEditRequest {
                    title: example_org::SEED_EDITOR_CHANGE_TITLE.to_owned(),
                    summary: "Says where the names come from.".into(),
                    changes: vec![PageChange {
                        path: path.to_owned(),
                        base_sha256: page.sha256,
                        base_markdown: page.markdown,
                        markdown: proposed,
                        delete: false,
                    }],
                    request_review: false,
                },
            )
            .map_err(|e| eyre::eyre!("open {}'s change: {e}", owner.name))?;
        println!(
            "  edit request: \"{}\" by {} {}",
            example_org::SEED_EDITOR_CHANGE_TITLE,
            owner.name,
            if made.auto_approved {
                "auto-approved"
            } else {
                "left open"
            }
        );
    }
    Ok(())
}

#[cfg(feature = "plugin-wiki")]
fn plant_core_subscriptions(org: &org_proto::OrgRoot) {
    let store = wiki_live::subscriptions::SubscriptionStore::open(org.path());
    let core = crate::core_subscriptions();
    let mut subscribers = vec![wiki_proto::Subscriber::Vault];
    subscribers.extend(
        org.named_wikis()
            .into_iter()
            .map(|(slug, _)| wiki_proto::Subscriber::Wiki(slug)),
    );
    let mut added = 0;
    for subscriber in &subscribers {
        match store.ensure_core(subscriber, &core) {
            Ok(new) => added += new.len(),
            Err(e) => {
                println!("  subscriptions: not applied ({e})");
                return;
            }
        }
    }
    println!(
        "  subscriptions: core set on {} subscriber(s), {added} new",
        subscribers.len()
    );
}
