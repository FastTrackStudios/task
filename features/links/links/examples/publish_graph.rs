//! Publish the **interconnect graph** — export only the shareable links
//! (`visibility >= Unlisted`), withholding the private journal entirely,
//! and redacting any private-note endpoints so a public edge can't leak a
//! note's path. This is the `knowledge-primitives.md` §5 publish path:
//! the published artifact is a graph of verse↔verse↔idea connections, not
//! your private content.
//!
//! Run: cargo run -p links --example publish_graph -- [ORG_ROOT]
//!   ORG_ROOT defaults to ~/.task/orgs/codywright

use std::path::PathBuf;

use std::collections::BTreeMap;

use links::Store;
use links_proto::{Confidence, LinksService, NodeKind, NodeRef, TypedLink};

fn confidence_label(c: Confidence) -> &'static str {
    match c {
        Confidence::Speculative => "speculative",
        Confidence::Unlikely => "unlikely",
        Confidence::Possible => "possible",
        Confidence::Likely => "likely",
        Confidence::Certain => "certain",
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A self-contained HTML page of the interconnect graph: edges grouped by
/// source node, each showing its relation, target, confidence + note. No
/// server, no JS — just open it.
fn render_html(links: &[TypedLink]) -> String {
    let mut by_source: BTreeMap<String, Vec<&TypedLink>> = BTreeMap::new();
    for l in links {
        by_source.entry(l.source.to_token()).or_default().push(l);
    }
    let mut body = String::new();
    for (source, edges) in &by_source {
        body.push_str(&format!("<section><h2>{}</h2><ul>", esc(source)));
        for l in edges {
            let note = if l.note.is_empty() {
                String::new()
            } else {
                format!(" <span class=\"note\">— {}</span>", esc(&l.note))
            };
            body.push_str(&format!(
                "<li><span class=\"rel\">{}</span> <span class=\"tgt\">{}</span> <span class=\"conf c-{}\">{}</span>{}</li>",
                esc(l.relation.as_str()),
                esc(&l.target.to_token()),
                confidence_label(l.confidence),
                confidence_label(l.confidence),
                note,
            ));
        }
        body.push_str("</ul></section>");
    }
    let nodes: std::collections::BTreeSet<String> = links
        .iter()
        .flat_map(|l| [l.source.to_token(), l.target.to_token()])
        .collect();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Published interconnections</title><style>\
:root{{color-scheme:light dark}}body{{font:15px/1.5 system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem}}\
h1{{font-size:1.4rem}}h2{{font-size:.95rem;font-family:ui-monospace,monospace;margin:1.4rem 0 .3rem;color:#2563eb}}\
ul{{list-style:none;padding-left:.5rem;margin:0}}li{{padding:.15rem 0;border-bottom:1px solid #8884}}\
.rel{{font-size:.72rem;text-transform:uppercase;letter-spacing:.04em;color:#888}}\
.tgt{{font-family:ui-monospace,monospace}}\
.conf{{font-size:.68rem;border:1px solid #8886;border-radius:4px;padding:0 .3rem;margin-left:.2rem}}\
.c-certain{{color:#16a34a}}.c-likely{{color:#0891b2}}.c-possible,.c-unlikely,.c-speculative{{color:#888}}\
.note{{color:#666}}.lede{{color:#666}}</style></head><body>\
<h1>Published interconnections</h1>\
<p class=\"lede\">{} links · {} nodes. Generated from the public (non-private) slice of the knowledge graph.</p>{}</body></html>",
        links.len(),
        nodes.len(),
        body,
    )
}

/// Redact a private-content endpoint: a `note:` node's id is the vault
/// path, so replace it with an opaque tag (keep the kind, drop the path).
fn redact(node: &NodeRef) -> NodeRef {
    if node.kind == NodeKind::Note {
        NodeRef::new(NodeKind::Note, "private")
    } else {
        node.clone()
    }
}

fn main() {
    let org: PathBuf = std::env::args().nth(1).map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap()).join(".task/orgs/codywright"),
        PathBuf::from,
    );

    let store = Store::open(org.join("links.jsonl"));
    // Everything, vs the publishable subset (Private links dropped).
    let all = store
        .graph(Confidence::Speculative, true)
        .expect("read links");
    let public = store
        .graph(Confidence::Speculative, false)
        .expect("read public links");

    // Redact private-note endpoints on the surviving public edges.
    let published: Vec<TypedLink> = public
        .iter()
        .map(|l| {
            let mut l = l.clone();
            l.source = redact(&l.source);
            l.target = redact(&l.target);
            l
        })
        .collect();

    let out_dir = org.join("published");
    std::fs::create_dir_all(&out_dir).expect("mkdir published");
    let mut out = String::new();
    for l in &published {
        out.push_str(&serde_json::to_string(l).expect("encode"));
        out.push('\n');
    }
    let path = out_dir.join("links.jsonl");
    std::fs::write(&path, out).expect("write published links");

    // Self-contained shareable page.
    let html_path = out_dir.join("index.html");
    std::fs::write(&html_path, render_html(&published)).expect("write published html");

    let withheld = all.len() - public.len();
    let redacted = published
        .iter()
        .filter(|l| l.source.id == "private" || l.target.id == "private")
        .count();
    println!(
        "published {} public links → {} + {} ({withheld} private withheld, {redacted} note endpoints redacted)",
        published.len(),
        path.display(),
        html_path.display()
    );
}
