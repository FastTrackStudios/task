//! `task api` — render the self-describing API reference.
//!
//! The CLI links `task-server`, so this renders
//! [`task_server::api_ref::reference`] — the SAME registry fold the
//! server serves at `GET /org/{slug}/api` and the permit gate installs.
//! No server or network involved: the output describes the build in
//! front of you (which is exactly what the committed markdown reference
//! should track).
//!
//! - `task api` — one line per service (name, alias, stream-ness,
//!   method count, schema stamp).
//! - `task api <service>` — one service's methods in full (args,
//!   permit action + resource, stream, audited). Matches the permit
//!   alias (`task`, `vault-sync`), the descriptor name
//!   (`TaskServiceRpc`), or a unique substring of either.
//! - `task api --markdown` — the whole reference as markdown; commit it
//!   as `docs/api-reference.md`.
//! - `task api --json` — the exact JSON body `GET /org/{slug}/api`
//!   serves (minus the `org` field the handler stamps in).

use clap::Args;
use task_server::api_ref::{ApiService, reference, reference_for};

#[derive(Args)]
pub(crate) struct ApiArgs {
    /// Show one service's methods (permit alias, descriptor name, or a
    /// unique substring of either).
    service: Option<String>,

    /// Emit the whole reference as markdown
    /// (`task api --markdown > docs/api-reference.md`).
    #[arg(long, conflicts_with = "json")]
    markdown: bool,

    /// Emit the reference as JSON — the same shape `GET
    /// /org/{slug}/api` serves.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run_api(args: ApiArgs) -> eyre::Result<()> {
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&task_server::api_ref::reference_json())?
        );
        return Ok(());
    }
    if args.markdown {
        // The committed reference is build-static: full catalog, no
        // org's deny-list applied.
        print!("{}", render_markdown(&reference()));
        return Ok(());
    }
    // Interactive views: apply the LOCAL active org's deny-list (when
    // one resolves without side effects) so disabled services are
    // marked — they still list; they just won't answer on the wire.
    let services = reference_for(&local_plugin_set());
    match args.service {
        Some(query) => render_one(&services, &query),
        None => {
            render_index(&services);
            Ok(())
        }
    }
}

/// The plugin set of the locally-active org, resolved WITHOUT side
/// effects (no auto-bootstrap — `task api` is a read-only, build-static
/// command). Falls back to "everything on" when no org resolves: the
/// session's active slug first, else the single local org.
fn local_plugin_set() -> task_plugin::PluginSet {
    let disabled = (|| {
        let root = org_proto::DataRoot::from_env().ok()?;
        let orgs = root.scan_orgs().ok()?;
        let active = crate::session_store::load()
            .ok()
            .flatten()
            .map(|s| s.active_slug())
            .filter(|s| !s.is_empty());
        let manifest = match active {
            Some(slug) => orgs.iter().find(|(o, _)| o.slug() == slug).map(|(_, m)| m),
            None if orgs.len() == 1 => Some(&orgs[0].1),
            None => None,
        }?;
        Some(manifest.disabled_plugins.0.clone())
    })();
    task_plugin::PluginSet::resolve(disabled.map(task_plugin::PluginChoice::Disabled).as_ref())
}

/// Find a service by alias, descriptor name, or unique substring.
fn find<'a>(services: &'a [ApiService], query: &str) -> Result<&'a ApiService, String> {
    let q = query.to_ascii_lowercase();
    // Exact alias or descriptor name first.
    if let Some(s) = services
        .iter()
        .find(|s| s.alias.is_some_and(|a| a == q) || s.name.to_ascii_lowercase() == q)
    {
        return Ok(s);
    }
    let matches: Vec<&ApiService> = services
        .iter()
        .filter(|s| {
            s.name.to_ascii_lowercase().contains(&q) || s.alias.is_some_and(|a| a.contains(&q))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no mounted service matches `{query}`")),
        many => Err(format!(
            "`{query}` is ambiguous — matches: {}",
            many.iter()
                .map(|s| s.alias.unwrap_or(s.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn render_index(services: &[ApiService]) {
    let streams = services.iter().filter(|s| s.stream).count();
    let disabled = services.iter().filter(|s| !s.mounted).count();
    println!(
        "{} services ({} streams, {} rpc{}). `task api <service>` for methods.",
        services.len(),
        streams,
        services.len() - streams,
        if disabled > 0 {
            format!(", {disabled} disabled for the active org")
        } else {
            String::new()
        },
    );
    println!();
    let width = services
        .iter()
        .map(|s| s.alias.unwrap_or(s.name).len())
        .max()
        .unwrap_or(0);
    let plugin_width = services.iter().map(|s| s.plugin.len()).max().unwrap_or(0);
    for s in services {
        println!(
            "  {:width$}  {:plugin_width$}  {:2} method(s)  {}  stamp {}   ({}){}",
            s.alias.unwrap_or(s.name),
            s.plugin,
            s.methods.len(),
            if s.stream { "stream" } else { "rpc   " },
            s.stamp,
            s.name,
            if s.mounted { "" } else { "  [DISABLED]" },
        );
    }
}

fn render_one(services: &[ApiService], query: &str) -> eyre::Result<()> {
    let s = find(services, query).map_err(|e| eyre::eyre!(e))?;
    println!(
        "{} ({}) — {}, plugin `{}`, stamp {}{}",
        s.alias.unwrap_or(s.name),
        s.name,
        if s.stream {
            "#[subscribe] stream service"
        } else {
            "rpc service"
        },
        s.plugin,
        s.stamp,
        if s.mounted {
            ""
        } else {
            "  [DISABLED for the active org — not mounted on its router]"
        },
    );
    if let Some(doc) = s.doc {
        println!("  {}", doc.trim());
    }
    println!();
    for m in &s.methods {
        let mut tags = Vec::new();
        if m.stream {
            tags.push("stream");
        }
        if m.audited {
            tags.push("audited");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(", "))
        };
        println!(
            "  {}({})  — {} on {}{}",
            m.name,
            m.args.join(", "),
            m.action.unwrap_or("?"),
            m.resource.unwrap_or("?"),
            tags,
        );
        if let Some(doc) = m.doc {
            if let Some(line) = doc.trim().lines().next() {
                println!("      {line}");
            }
        }
    }
    Ok(())
}

fn render_markdown(services: &[ApiService]) -> String {
    use std::fmt::Write as _;
    let streams = services.iter().filter(|s| s.stream).count();
    let mut out = String::new();
    let _ = writeln!(out, "# Task API reference");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "<!-- GENERATED — do not edit by hand. Regenerate with:\n\
         \x20    cargo run -p task-cli -- api --markdown > docs/api-reference.md\n\
         \x20    Source: `task_server::permits::mounts()` (apps/server/src/permits.rs),\n\
         \x20    the single registry the router, permit gate, and schema stamps derive from.\n\
         \x20    Served live at `GET /org/{{slug}}/api`. -->"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} services mounted: {} plain RPC, {} `#[subscribe]` streams. Every method \
         lists its permit — the `<action>` on `<resource>` the permissions gate \
         checks (see `apps/server/src/permits.rs`). `audited` methods emit an \
         audit line even when allowed. A `stream` method takes a `Tx` sink and \
         pushes to the caller instead of returning once.",
        services.len(),
        services.len() - streams,
        streams,
    );
    let _ = writeln!(out);
    for s in services {
        let _ = writeln!(
            out,
            "## `{}` ({}){}",
            s.alias.unwrap_or(s.name),
            s.name,
            if s.stream { " — stream" } else { "" }
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "Plugin: `{}` — schema stamp: `{}`", s.plugin, s.stamp);
        let _ = writeln!(out);
        let _ = writeln!(out, "| method | args | permit | notes |");
        let _ = writeln!(out, "|---|---|---|---|");
        for m in &s.methods {
            let mut notes = Vec::new();
            if m.stream {
                notes.push("stream");
            }
            if m.audited {
                notes.push("audited");
            }
            let _ = writeln!(
                out,
                "| `{}` | {} | `{}` on `{}` | {} |",
                m.name,
                if m.args.is_empty() {
                    "—".to_owned()
                } else {
                    m.args
                        .iter()
                        .map(|a| format!("`{a}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                m.action.unwrap_or("?"),
                m.resource.unwrap_or("?"),
                if notes.is_empty() {
                    "—".to_owned()
                } else {
                    notes.join(", ")
                },
            );
        }
        let _ = writeln!(out);
    }
    out
}
