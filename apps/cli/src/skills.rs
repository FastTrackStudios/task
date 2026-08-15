//! `task skills install` — write the agent-lane skills into a working
//! copy, parameterised for this org and project.
//!
//! The parameterisation is the whole reason this is a CLI command
//! rather than `cp -r`: a generic installer cannot know your org
//! slug, your project, or the verify command a ticket here will
//! inherit. Guidance that names them is guidance you can follow
//! without translating it first.

use std::path::{Path, PathBuf};

use clap::Subcommand;

use crate::resolve_active_org;
use crate::resolve_org_vox_url;

/// The skills as they live in the repo.
///
/// Embedded rather than read from disk so an installed `task` binary
/// carries them — the command has to work from a checkout that is not
/// this monorepo.
static SKILLS: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../skills/agent-lane");

#[derive(Subcommand, Debug)]
pub enum SkillsCmd {
    /// Write the skills into a working copy.
    Install {
        /// Where to write them. Default: `.claude/skills` here.
        #[arg(long)]
        into: Option<PathBuf>,
        /// Project whose verify command the guidance should name.
        #[arg(long)]
        project: Option<String>,
        /// Overwrite files that already exist.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// List what would be installed.
    List,
}

pub(crate) async fn run_skills(cmd: SkillsCmd) -> eyre::Result<()> {
    match cmd {
        SkillsCmd::List => {
            for entry in SKILLS.dirs() {
                println!("{}", entry.path().display());
            }
            for f in SKILLS.files() {
                println!("{}", f.path().display());
            }
        }

        SkillsCmd::Install {
            into,
            project,
            force,
            org,
            server,
        } => {
            let slug = resolve_active_org(org.clone())?;
            let url = resolve_org_vox_url(server, &slug);

            // Resolve the context the guidance will name. A failure
            // here is not fatal — skills that name nothing are still
            // better than no skills — but it is worth reporting,
            // because unparameterised guidance is what we are trying
            // to avoid.
            let (project_name, verify) = match resolve_context(&url, project.as_deref()).await {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("warning: installing without project context: {e}");
                    (String::new(), String::new())
                }
            };

            let root = into.unwrap_or_else(|| PathBuf::from(".claude/skills"));
            std::fs::create_dir_all(&root)?;

            let ctx = Context {
                org: slug.clone(),
                project: project_name,
                verify,
            };

            let mut written = 0_usize;
            let mut skipped = 0_usize;
            write_dir(&SKILLS, &root, &ctx, force, &mut written, &mut skipped)?;

            println!("installed {written} file(s) into {}", root.display());
            if skipped > 0 {
                println!("skipped {skipped} existing file(s) — pass --force to overwrite");
            }
            println!("  org:     {}", ctx.org);
            if ctx.project.is_empty() {
                println!("  project: (none — guidance is generic)");
            } else {
                println!("  project: {}", ctx.project);
                println!("  verify:  {}", ctx.verify);
            }
        }
    }
    Ok(())
}

struct Context {
    org: String,
    project: String,
    verify: String,
}

/// The org's project and the verify command its tickets inherit.
async fn resolve_context(url: &str, project: Option<&str>) -> eyre::Result<(String, String)> {
    let pc = crate::project::connect_project_client(url).await?;
    let projects = pc
        .list()
        .await
        .map_err(|e| eyre::eyre!("list projects: {e:?}"))?;

    let Some(want) = project else {
        return Ok((String::new(), String::new()));
    };
    let hit = projects
        .iter()
        .find(|p| {
            p.id.to_string().starts_with(want)
                || p.title.eq_ignore_ascii_case(want)
                || p.path == want
        })
        .ok_or_else(|| eyre::eyre!("no project matching `{want}`"))?;

    let verify = project::verify::project_default(Some(hit.id), &projects).unwrap_or_default();
    Ok((hit.title.clone(), verify))
}

/// Copy a directory in, substituting context as we go.
fn write_dir(
    dir: &include_dir::Dir<'_>,
    root: &Path,
    ctx: &Context,
    force: bool,
    written: &mut usize,
    skipped: &mut usize,
) -> eyre::Result<()> {
    for f in dir.files() {
        let out = root.join(f.path());
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if out.exists() && !force {
            *skipped += 1;
            continue;
        }
        let body = String::from_utf8_lossy(f.contents());
        std::fs::write(&out, substitute(&body, ctx))?;
        *written += 1;
    }
    for d in dir.dirs() {
        write_dir(d, root, ctx, force, written, skipped)?;
    }
    Ok(())
}

/// Fill the placeholders a skill may carry.
///
/// Unset context leaves a readable fallback rather than an empty
/// hole — guidance reading `--org <your org>` is still usable, where
/// `--org ` is a footgun.
fn substitute(body: &str, ctx: &Context) -> String {
    let project = if ctx.project.is_empty() {
        "<your project>"
    } else {
        &ctx.project
    };
    let verify = if ctx.verify.is_empty() {
        "<your verify command>"
    } else {
        &ctx.verify
    };
    body.replace("{{ORG}}", &ctx.org)
        .replace("{{PROJECT}}", project)
        .replace("{{VERIFY}}", verify)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            org: "fasttrackstudios".into(),
            project: "Task".into(),
            verify: "cargo check -p task".into(),
        }
    }

    #[test]
    fn placeholders_are_filled() {
        let out = substitute(
            "--org {{ORG}} --project {{PROJECT}} --verify {{VERIFY}}",
            &ctx(),
        );
        assert_eq!(
            out,
            "--org fasttrackstudios --project Task --verify cargo check -p task"
        );
    }

    #[test]
    fn missing_context_leaves_something_readable() {
        let bare = Context {
            org: "solo".into(),
            project: String::new(),
            verify: String::new(),
        };
        let out = substitute("{{PROJECT}} / {{VERIFY}}", &bare);
        assert_eq!(out, "<your project> / <your verify command>");
        assert!(
            !out.contains("{{"),
            "an unsubstituted placeholder would leak into guidance"
        );
    }

    #[test]
    fn text_without_placeholders_is_untouched() {
        let body = "# Wayfinder\n\nA loose idea has arrived.\n";
        assert_eq!(substitute(body, &ctx()), body);
    }

    #[test]
    fn the_skills_are_actually_embedded() {
        // Guards the include_dir path: a wrong one compiles happily
        // and installs nothing.
        assert!(
            SKILLS.dirs().count() > 5,
            "expected the forked skill set to be embedded"
        );
        assert!(
            SKILLS.get_file("ISSUE-TRACKER.md").is_some(),
            "the tracker adapter is the seam every skill reads"
        );
    }
}
