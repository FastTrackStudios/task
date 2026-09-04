//! The vault as it exists in a *built* site: `&'static` everything.
//!
//! [`Page`] is the build-time form — owned `String`s, produced by
//! scanning a directory. This is what that lowers to once `ssg-build`
//! has codegen'd it into the consuming crate: the same fields as
//! `&'static str`, so a running site does no allocation, no parsing and
//! no I/O to show a page. Every byte is in `.rodata` and the render is a
//! slice lookup.
//!
//! Both types exist because they have genuinely different jobs, and
//! collapsing them would mean either allocating at runtime or making the
//! scanner generic over ownership for no gain.

/// One heading in a built page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticHeading {
    /// `1` for `#`, `2` for `##`, and so on.
    pub level: u8,
    /// The heading's text, markup flattened away.
    pub text: &'static str,
    /// Its `id` in the rendered HTML — what `#fragment` addresses it.
    pub id: &'static str,
}

/// One page of a built vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticPage {
    /// URL segment, and the `[[wikilink]]` target.
    pub slug: &'static str,
    /// Display title.
    pub title: &'static str,
    /// One line for a table of contents. Empty when the note has none.
    pub summary: &'static str,
    /// Sort key. Pages without one were given `u32::MAX` and sort last.
    pub order: u32,
    /// The table-of-contents section this note sits under. Empty when
    /// the vault has no stages.
    pub stage: &'static str,
    /// Frontmatter `type:`, lowercased. `"other"` when absent.
    pub kind: &'static str,
    /// The note verbatim. The knowledge graph reads this; a reader does
    /// not.
    pub source: &'static str,
    /// The note's markdown without its frontmatter — the prose as text,
    /// for a consumer that wants to edit or re-render it rather than
    /// display it.
    pub body: &'static str,
    /// The note as finished HTML — no frontmatter, no nav footer,
    /// wikilinks resolved, fences expanded.
    pub html: &'static str,
    /// Outbound wikilink targets that resolved, in document order.
    pub links: &'static [&'static str],
    /// The page's headings, in document order.
    pub headings: &'static [StaticHeading],
    /// Frontmatter `tags:`, lowercased.
    pub tags: &'static [&'static str],
    /// Words of prose.
    pub words: u32,
}

impl StaticPage {
    /// Minutes to read, at 220 words a minute, never less than one.
    ///
    /// 220 is a middling estimate for adult reading of technical prose;
    /// the number is a courtesy, not a measurement, and rounding a
    /// 40-word note to "0 min" would be a worse lie than rounding it up.
    #[must_use]
    pub const fn reading_minutes(&self) -> u32 {
        let minutes = self.words / 220;
        if minutes == 0 { 1 } else { minutes }
    }
}

/// A built vault: its pages in reading order.
///
/// The methods here are the same reads every site was writing by hand —
/// find a page, find what links to it, walk to the next chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticVault {
    /// Pages sorted by `(order, slug)`.
    pub pages: &'static [StaticPage],
}

impl StaticVault {
    /// Wrap a codegen'd page table.
    #[must_use]
    pub const fn new(pages: &'static [StaticPage]) -> Self {
        Self { pages }
    }

    /// Look up a page by slug.
    #[must_use]
    pub fn page(&self, slug: &str) -> Option<&'static StaticPage> {
        self.pages.iter().find(|p| p.slug == slug)
    }

    /// The front door — first in reading order.
    ///
    /// `Option` even though `ssg-build` refuses to generate an empty
    /// vault: "this cannot happen" is exactly the claim that ages badly,
    /// and the caller can decide what an empty guide looks like.
    #[must_use]
    pub fn first(&self) -> Option<&'static StaticPage> {
        self.pages.first()
    }

    /// Whether the vault is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// The pages linking *to* `slug`, in reading order.
    //
    // t[impl ssg.order.backlinks] — derived from the outbound links every
    // page recorded at build time; nothing declares a backlink.
    #[must_use]
    pub fn backlinks(&self, slug: &str) -> Vec<&'static StaticPage> {
        self.pages
            .iter()
            .filter(|p| p.links.contains(&slug))
            .collect()
    }

    /// The page before `slug` in reading order.
    #[must_use]
    pub fn previous(&self, slug: &str) -> Option<&'static StaticPage> {
        let at = self.index_of(slug)?;
        self.pages.get(at.checked_sub(1)?)
    }

    /// The page after `slug` in reading order.
    #[must_use]
    pub fn next(&self, slug: &str) -> Option<&'static StaticPage> {
        self.pages.get(self.index_of(slug)?.checked_add(1)?)
    }

    /// Position of `slug` in reading order.
    #[must_use]
    pub fn index_of(&self, slug: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.slug == slug)
    }

    /// The vault's stages, in reading order, each with its pages.
    ///
    /// A stage heading is emitted wherever the `stage:` value changes,
    /// so the order of the headings *is* the reading order and a page
    /// cannot appear under a stage it does not belong to. A vault with
    /// no stages comes back as one unnamed group.
    #[must_use]
    pub fn stages(&self) -> Vec<(&'static str, Vec<&'static StaticPage>)> {
        let mut groups: Vec<(&'static str, Vec<&'static StaticPage>)> = Vec::new();
        for page in self.pages {
            match groups.last_mut() {
                Some((stage, pages)) if *stage == page.stage => pages.push(page),
                _ => groups.push((page.stage, vec![page])),
            }
        }
        groups
    }

    // t[impl ssg.order.tags]
    /// Every tag in the vault, alphabetical, with how many pages carry
    /// it.
    ///
    /// The vault's other axis. Reading order is one path through it —
    /// the one the author chose — and tags are the cross-cutting one: a
    /// reader who wants "everything about tempo" is not asking for a
    /// chapter range.
    #[must_use]
    pub fn tags(&self) -> Vec<(&'static str, usize)> {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for tag in self.pages.iter().flat_map(|p| p.tags) {
            match counts.iter_mut().find(|(name, _)| name == tag) {
                Some((_, count)) => *count += 1,
                None => counts.push((tag, 1)),
            }
        }
        counts.sort_unstable_by(|a, b| a.0.cmp(b.0));
        counts
    }

    /// The pages carrying `tag`, in reading order.
    #[must_use]
    pub fn tagged(&self, tag: &str) -> Vec<&'static StaticPage> {
        self.pages
            .iter()
            .filter(|p| p.tags.contains(&tag))
            .collect()
    }

    // t[impl ssg.output.routes]
    /// Every route this vault publishes, as URL paths under `base`.
    ///
    /// This is what a site hands back from its `static_routes` server
    /// function: `dx build --ssg` asks the running server for the list of
    /// paths to bake, and a dynamic route like `/guide/:slug` cannot be
    /// enumerated from the route table alone — only the vault knows its
    /// own slugs. Handing this over is the whole of "partial" static
    /// generation: these paths are baked, everything else stays live.
    #[must_use]
    pub fn routes(&self, base: &str) -> Vec<String> {
        let base = base.trim_end_matches('/');
        std::iter::once(base.to_owned())
            .chain(self.pages.iter().map(|p| format!("{base}/{}", p.slug)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn page(
        slug: &'static str,
        stage: &'static str,
        links: &'static [&'static str],
    ) -> StaticPage {
        StaticPage {
            slug,
            title: slug,
            summary: "",
            order: 0,
            stage,
            kind: "other",
            source: "",
            body: "",
            html: "",
            headings: &[],
            tags: &[],
            words: 0,
            links,
        }
    }

    static PAGES: &[StaticPage] = &[
        page("intro", "Start here", &["chords"]),
        page("chords", "Start here", &["rhythm"]),
        page("rhythm", "The music", &["chords"]),
    ];

    fn vault() -> StaticVault {
        StaticVault::new(PAGES)
    }

    #[test]
    fn walks_forward_and_back_in_reading_order() {
        assert_eq!(vault().next("intro").expect("next").slug, "chords");
        assert_eq!(vault().previous("chords").expect("prev").slug, "intro");
        assert!(vault().previous("intro").is_none());
        assert!(vault().next("rhythm").is_none());
    }

    static TAGGED: &[StaticPage] = &[
        StaticPage {
            tags: &["tempo", "rig"],
            ..page("intro", "", &[])
        },
        StaticPage {
            tags: &["tempo"],
            ..page("chords", "", &[])
        },
        StaticPage {
            words: 660,
            ..page("rhythm", "", &[])
        },
    ];

    // t[verify ssg.order.tags]
    #[test]
    fn tags_are_counted_across_the_vault_and_sorted() {
        let vault = StaticVault::new(TAGGED);
        assert_eq!(vault.tags(), vec![("rig", 1), ("tempo", 2)]);
    }

    #[test]
    fn a_tag_lists_the_pages_carrying_it_in_reading_order() {
        let vault = StaticVault::new(TAGGED);
        let slugs: Vec<_> = vault.tagged("tempo").iter().map(|p| p.slug).collect();
        assert_eq!(slugs, ["intro", "chords"]);
        assert!(vault.tagged("nothing").is_empty());
    }

    #[test]
    fn a_reading_estimate_never_rounds_to_nothing() {
        let vault = StaticVault::new(TAGGED);
        // 660 words at 220 a minute.
        assert_eq!(vault.page("rhythm").expect("page").reading_minutes(), 3);
        // A note of no words is still a one-minute read, not a
        // zero-minute one.
        assert_eq!(vault.page("intro").expect("page").reading_minutes(), 1);
    }

    #[test]
    fn backlinks_find_every_referrer() {
        let back: Vec<_> = vault().backlinks("chords").iter().map(|p| p.slug).collect();
        assert_eq!(back, ["intro", "rhythm"]);
    }

    #[test]
    fn stages_group_consecutive_pages() {
        let stages = vault().stages();
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].0, "Start here");
        assert_eq!(stages[0].1.len(), 2);
        assert_eq!(stages[1].0, "The music");
    }

    // t[verify ssg.output.routes]
    #[test]
    fn routes_include_the_index_and_every_page() {
        assert_eq!(
            vault().routes("/guide/"),
            ["/guide", "/guide/intro", "/guide/chords", "/guide/rhythm"]
        );
    }
}
