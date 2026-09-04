//! A sitemap and a feed, from the vault.
//!
//! Both are the same observation twice: a statically generated site is a
//! known, finite list of URLs with known content, so the two files that
//! say so can be written at build time and cost nothing to keep in step.
//!
//! Deliberately plain string building rather than an XML crate. The
//! documents have four element types between them, the only hard part is
//! escaping, and a dependency here is paid for by every consuming
//! workspace on every cold build.

use crate::Vault;

// t[impl ssg.output.feeds]
/// An `urlset` naming every page of the vault.
///
/// `site` is the origin with no trailing slash (`https://keyflow.example`)
/// and `base` the vault's path under it (`/guide`).
///
/// No `lastmod`, `changefreq` or `priority`. The first needs a date this
/// crate does not have — a caller with git history can add it — and
/// search engines have said for years that the other two are ignored.
#[must_use]
pub fn sitemap(vault: &Vault, site: &str, base: &str) -> String {
    let site = site.trim_end_matches('/');
    let base = base.trim_end_matches('/');

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for page in &vault.pages {
        out.push_str("  <url><loc>");
        out.push_str(&escape_xml(&format!("{site}{base}/{}", page.slug)));
        out.push_str("</loc></url>\n");
    }
    out.push_str("</urlset>\n");
    out
}

/// An RSS 2.0 channel of the vault's pages, in reading order.
///
/// Reading order rather than newest-first, because a guide is a sequence
/// and has no dates. That makes this a "here is the whole guide" feed
/// rather than a "what changed" one — which is the honest thing to
/// publish for content that is edited rather than posted.
#[must_use]
pub fn rss(vault: &Vault, site: &str, base: &str, title: &str, description: &str) -> String {
    let site = site.trim_end_matches('/');
    let base = base.trim_end_matches('/');

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\">\n  <channel>\n",
    );
    let _ = write_element(&mut out, "title", title);
    let _ = write_element(&mut out, "link", &format!("{site}{base}"));
    let _ = write_element(&mut out, "description", description);

    for page in &vault.pages {
        let url = format!("{site}{base}/{}", page.slug);
        out.push_str("    <item>\n");
        out.push_str("  ");
        let _ = write_element(&mut out, "title", &page.title);
        out.push_str("  ");
        let _ = write_element(&mut out, "link", &url);
        // The URL as the id, because a vault page has no other stable
        // one. `isPermaLink="true"` is the default and says so.
        out.push_str("  ");
        let _ = write_element(&mut out, "guid", &url);
        if !page.summary.is_empty() {
            out.push_str("  ");
            let _ = write_element(&mut out, "description", &page.summary);
        }
        out.push_str("    </item>\n");
    }

    out.push_str("  </channel>\n</rss>\n");
    out
}

fn write_element(out: &mut String, name: &str, text: &str) -> std::fmt::Result {
    use std::fmt::Write as _;
    writeln!(out, "    <{name}>{}</{name}>", escape_xml(text))
}

/// The five characters XML reserves.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Page;

    fn page(slug: &str, title: &str, summary: &str) -> Page {
        Page {
            slug: slug.to_owned(),
            title: title.to_owned(),
            summary: summary.to_owned(),
            order: 0,
            stage: String::new(),
            kind: "other".to_owned(),
            source: String::new(),
            body: String::new(),
            html: String::new(),
            headings: Vec::new(),
            tags: Vec::new(),
            words: 0,
            links: Vec::new(),
            broken_links: Vec::new(),
        }
    }

    fn vault() -> Vault {
        Vault {
            pages: vec![
                page("intro", "Intro", "Start here"),
                page("chords", "Chords & keys", "Naming <them>"),
            ],
        }
    }

    // t[verify ssg.output.feeds]
    #[test]
    fn a_sitemap_names_every_page_absolutely() {
        let xml = sitemap(&vault(), "https://keyflow.example/", "/guide/");
        assert!(xml.contains("<loc>https://keyflow.example/guide/intro</loc>"));
        assert!(xml.contains("<loc>https://keyflow.example/guide/chords</loc>"));
        // A trailing slash on either argument must not double up.
        assert!(!xml.contains("//guide"));
        assert_eq!(xml.matches("<url>").count(), 2);
    }

    #[test]
    fn xml_reserved_characters_are_escaped() {
        let xml = rss(&vault(), "https://x.example", "/guide", "G & G", "A <feed>");
        assert!(xml.contains("<title>G &amp; G</title>"));
        assert!(xml.contains("<description>A &lt;feed&gt;</description>"));
        assert!(xml.contains("<title>Chords &amp; keys</title>"));
        assert!(xml.contains("Naming &lt;them&gt;"));
        // Nothing unescaped survived into the document.
        assert!(!xml.contains("G & G"));
    }

    #[test]
    fn a_feed_item_carries_a_stable_id() {
        let xml = rss(&vault(), "https://x.example", "/guide", "G", "");
        assert!(xml.contains("<guid>https://x.example/guide/intro</guid>"));
        assert_eq!(xml.matches("<item>").count(), 2);
    }

    #[test]
    fn a_page_with_no_summary_omits_the_description() {
        let mut v = vault();
        v.pages[0].summary = String::new();
        let xml = rss(&v, "https://x.example", "/guide", "G", "d");
        // The channel's own description, and the second page's — not a
        // hollow one for the first.
        assert_eq!(xml.matches("<description>").count(), 2);
    }
}
