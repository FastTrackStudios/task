//! Read a vault and report what a build would make of it.
//!
//! ```text
//! cargo run -p ssg-vault --example lint-vault -- ../keyflow/docs/guides/keyflow
//! ```
//!
//! Same scan and same renderer a site's build script runs, so this
//! answers the question a build script can only answer by failing:
//! which notes are in reading order, what links to what, and which
//! cross-references point at nothing. Useful before moving a vault,
//! after renaming a note, or from CI as a link check that does not need
//! the site to build.
//!
//! Exits non-zero when a cross-reference is broken.

fn main() -> std::process::ExitCode {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: lint-vault <vault directory>");
        return std::process::ExitCode::FAILURE;
    };

    let vault = match ssg_vault::scan_with(&dir, |slugs| ssg_vault::Renderer::new("/guide", slugs))
    {
        Ok(vault) => vault,
        Err(err) => {
            eprintln!("error: {err}");
            return std::process::ExitCode::FAILURE;
        }
    };

    println!("{dir}\n{} pages, in reading order:\n", vault.pages.len());
    let mut stage = "";
    for page in &vault.pages {
        if page.stage != stage {
            stage = &page.stage;
            if !stage.is_empty() {
                println!("  [{stage}]");
            }
        }
        println!(
            "  {:<28} {:>5} bytes html, {} link(s) out, {} in",
            page.slug,
            page.html.len(),
            page.links.len(),
            vault.backlinks(&page.slug).len(),
        );
    }

    let orphans: Vec<&str> = vault
        .pages
        .iter()
        .filter(|p| p.links.is_empty() && vault.backlinks(&p.slug).is_empty())
        .map(|p| p.slug.as_str())
        .collect();
    if !orphans.is_empty() {
        // Not an error. A vault can legitimately hold a page nothing
        // points at — a front door usually is one — but a page that
        // neither links nor is linked is worth a look.
        println!("\nunlinked pages: {}", orphans.join(", "));
    }

    let broken = vault.broken_links();
    if broken.is_empty() {
        println!("\nno broken cross-references");
        return std::process::ExitCode::SUCCESS;
    }

    println!("\n{} broken cross-reference(s):", broken.len());
    for (from, to) in broken {
        println!("  {from}.md → [[{to}]]");
    }
    std::process::ExitCode::FAILURE
}
