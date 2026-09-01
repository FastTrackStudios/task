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

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
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

    // The projects, DECLARED — pages in the org vault, which is what
    // the app's Projects view lists. Idempotent the same way the tree
    // planting is: a page already at the path is left alone.
    let projects = project::ProjectBackend::new(org.vault_dir());
    for declared in example_org::declared_of(&slug) {
        use project::ProjectService as _;
        let info = project::ProjectInfo {
            title: declared.title.to_owned(),
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
        let Some(info) = projects
            .list()
            .ok()
            .and_then(|all| all.into_iter().find(|p| p.title == declared.title))
        else {
            continue;
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
            tasks_backend
                .create(t)
                .map_err(|e| eyre::eyre!("seed task \"{title}\": {e}"))?;
            seeded += 1;
        }
        if seeded > 0 {
            println!("  tasks: {seeded} seeded for {}", declared.title);
        }
    }

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
            let dest = org
                .path()
                .join("files")
                .join("Projects")
                .join(declared.dir)
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
            let dest = org
                .path()
                .join("files")
                .join("Projects")
                .join(declared.dir)
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
        use files::service::roots::{AdoptRequest, RootsService};
        let backend = files::FilesBackend::new(org.path().join("files"), org.vault_dir())
            .map_err(|e| eyre::eyre!("files backend: {e}"))?;
        let existing = RootsService::list(&backend)
            .await
            .map_err(|e| eyre::eyre!("list roots: {e}"))?;
        for declared in example_org::declared_of(&slug) {
            if existing.iter().any(|r| r.name == declared.title) {
                continue;
            }
            let dir = org.path().join("files").join("Projects").join(declared.dir);
            if !dir.is_dir() {
                continue;
            }
            let root = backend
                .adopt(AdoptRequest {
                    path: dir.to_string_lossy().into_owned(),
                    name: declared.title.to_owned(),
                    flavor: files::model::RootFlavor::Media,
                    hash_content: true,
                })
                .await
                .map_err(|e| eyre::eyre!("adopt {}: {e}", declared.title))?;
            backend.settled(files::RootId::new(root.id)).await;
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
                if let Err(e) = files::FilesService::checkpoint_now(
                    &backend,
                    root.id,
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
                match files::FilesService::checkpoint_now(
                    &backend,
                    root.id,
                    Some(format!("{name} — final")),
                )
                .await
                {
                    Ok(_) => println!("  video: \"{name}\" finalled — two versions on record"),
                    Err(e) => println!("  video: final of \"{name}\" not checkpointed ({e})"),
                }
            }
        }
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
    let projects = org.path().join("files").join("Projects");
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

/// Which pass of a deliverable synth this is — the two must LOOK
/// different, or the review screen's compare demonstrates nothing.
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
}
