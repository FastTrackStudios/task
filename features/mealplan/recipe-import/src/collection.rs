//! Collections — `task recipe import-collection <listing-url>`.
//!
//! A *collection* is a listing page (an Allrecipes taxonomy page, an
//! author archive, a saved bookmarks export…) whose links point at
//! recipe pages. Importing one means: enumerate the recipe links,
//! skip the ones the cookbook already holds (matched on the canonical
//! source URL), run each remaining page through the ordinary
//! fetch → extract → synthesize pipeline, and stamp the result as a
//! *resource* — an imported reference recipe, distinct from the
//! curated ones a cook has actually made and annotated.
//!
//! Everything here is pure (no network, no filesystem): the CLI owns
//! the fetch loop, the politeness delay and the cookbook client. That
//! keeps enumeration, stamping and idempotence unit-testable from a
//! saved listing.
//!
//! What we learned about the Allrecipes listing that motivated this:
//! the Food Wishes page is a static Dotdash "taxonomy" listing —
//! every card is server-rendered as `<a data-doc-id href=…>`, there
//! is no pagination (`?page=N` redirects to the page itself) and
//! scrolling triggers no XHR. Recipe cards use two URL shapes,
//! `/recipe/<id>/<slug>/` (legacy) and `/<slug>-recipe-<id>` (current);
//! the other cards are round-up articles and galleries, which
//! [`is_recipe_url`] rejects so they never reach the extractor.

use std::collections::{BTreeMap, BTreeSet};

use scraper::{Html, Selector};

use crate::normalized::NormalizedRecipe;

// ── Enumeration ──────────────────────────────────────────────

/// Recipe links in a listing, in page order, deduplicated by
/// [`canonical_url`]. Accepts either a saved HTML listing or a plain
/// text file of one URL per line (blank lines and `#` comments
/// ignored) — the shape `--from-file` takes.
#[must_use]
pub fn enumerate(text: &str, base_url: &str) -> Vec<String> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('<') {
        recipe_links(text, base_url)
    } else {
        parse_url_list(text)
    }
}

/// Every `<a href>` in `html` that [`is_recipe_url`] accepts, made
/// absolute against `base_url`, canonicalized, deduplicated.
#[must_use]
pub fn recipe_links(html: &str, base_url: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let Ok(sel) = Selector::parse("a[href]") else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for a in doc.select(&sel) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Some(abs) = absolutize(href, base_url) else {
            continue;
        };
        if !is_recipe_url(&abs) {
            continue;
        }
        let canon = canonical_url(&abs);
        if seen.insert(canon.clone()) {
            out.push(canon);
        }
    }
    out
}

/// One URL per line; blank lines and `#` comments skipped.
#[must_use]
pub fn parse_url_list(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with("http://") && !line.starts_with("https://") {
            continue;
        }
        let canon = canonical_url(line);
        if seen.insert(canon.clone()) {
            out.push(canon);
        }
    }
    out
}

/// Does this URL point at a single recipe page (as opposed to a
/// round-up article, a gallery, or a category)?
///
/// Recognizes the two Allrecipes shapes and the generic
/// `/recipe/<slug>` / `/recipes/<slug>` path most recipe sites use.
/// Deliberately conservative — a false negative costs one recipe, a
/// false positive costs a wasted fetch *and* a confusing failure line.
#[must_use]
pub fn is_recipe_url(url: &str) -> bool {
    let path = path_of(url);
    let path = path.trim_end_matches('/');
    // Allrecipes legacy: /recipe/221093/good-frickin-paprika-chicken
    if let Some(rest) = path.strip_prefix("/recipe/") {
        let (id, slug) = rest.split_once('/').unwrap_or((rest, ""));
        return !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !slug.is_empty();
    }
    // Allrecipes current: /amish-apple-fritter-bread-recipe-11859335
    if let Some((_, id)) = path.rsplit_once("-recipe-") {
        return path.matches('/').count() == 1
            && !id.is_empty()
            && id.chars().all(|c| c.is_ascii_digit());
    }
    // Generic: /recipes/<slug> on other sites — but never a bare
    // category (`/recipes/`) or a nested taxonomy (`/recipes/16791/…`).
    if let Some(rest) = path.strip_prefix("/recipes/") {
        return !rest.is_empty()
            && !rest.contains('/')
            && !rest.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// The URL used as the recipe's identity: scheme + lowercased host +
/// path, without query string or fragment (tracking parameters and
/// `#comments` anchors would otherwise make the same page look new
/// every run).
#[must_use]
pub fn canonical_url(url: &str) -> String {
    let url = url.trim();
    let url = url.split(['#', '?']).next().unwrap_or(url);
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let scheme = scheme.to_ascii_lowercase();
    if path.is_empty() {
        format!("{scheme}://{host}/")
    } else {
        format!("{scheme}://{host}/{path}")
    }
}

/// `allrecipes` for `https://www.allrecipes.com/…` — the value of the
/// `source_site` metadata key.
#[must_use]
pub fn site_of(url: &str) -> String {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    host.split('.').next().unwrap_or(host).to_string()
}

/// The file a saved copy of `url` is expected under in a
/// `--pages-dir`: the last non-empty path segment plus `.html`.
/// Both Allrecipes shapes yield the slug, which is what a human
/// saving pages would name them anyway.
#[must_use]
pub fn page_file_name(url: &str) -> String {
    let canon = canonical_url(url);
    let path = path_of(&canon);
    let slug = path
        .split('/')
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or("recipe");
    format!("{slug}.html")
}

fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    match rest.find('/') {
        Some(i) => &rest[i..],
        None => "/",
    }
}

fn absolutize(href: &str, base: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split('/').next()?;
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("{scheme}://{rest}"));
    }
    if let Some(rest) = href.strip_prefix('/') {
        return Some(format!("{scheme}://{host}/{rest}"));
    }
    // Relative to the listing's directory.
    let base_dir = base.rsplit_once('/').map_or(base, |(d, _)| d);
    Some(format!("{base_dir}/{href}"))
}

// ── Idempotence ──────────────────────────────────────────────

/// Which existing recipe (by path) already carries `url` as its
/// source, if any. Comparison is on [`canonical_url`] so a re-run
/// against a listing whose links grew tracking parameters still
/// matches.
pub fn find_present<'a, I>(existing: I, url: &str) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, Option<&'a str>)>,
{
    let want = canonical_url(url);
    existing
        .into_iter()
        .find(|(_, src)| src.is_some_and(|s| canonical_url(s) == want))
        .map(|(path, _)| path.to_string())
}

// ── Stamping ─────────────────────────────────────────────────

/// The provenance a collection import writes into every recipe.
#[derive(Debug, Clone)]
pub struct ResourceStamp {
    /// Human name of the collection ("Food Wishes").
    pub collection: String,
    /// `source_site` value ("allrecipes"); see [`site_of`].
    pub site: String,
    /// Author when the page's structured data carried none.
    pub author_fallback: Option<String>,
    /// `YYYY-MM-DD` of the run.
    pub imported: String,
    /// Tags to add (merged with whatever the page already had).
    pub tags: Vec<String>,
}

/// Cooklang metadata keys a resource import guarantees.
pub const SOURCE_SITE_KEY: &str = "source_site";
pub const COLLECTION_KEY: &str = "collection";
pub const IMPORTED_KEY: &str = "imported";
pub const CURATED_KEY: &str = "curated";

/// Stamp `recipe` as an imported resource: canonical source URL,
/// `source_site`, `collection`, `author` (page's, else the fallback),
/// `imported`, merged `tags`, and `curated: false`.
pub fn stamp_resource(recipe: &mut NormalizedRecipe, stamp: &ResourceStamp) {
    if let Some(u) = &recipe.source_url {
        recipe.source_url = Some(canonical_url(u));
    }
    let m = &mut recipe.metadata;
    m.insert(SOURCE_SITE_KEY.into(), stamp.site.clone());
    m.insert(COLLECTION_KEY.into(), stamp.collection.clone());
    if !m.get("author").is_some_and(|a| !a.trim().is_empty()) {
        if let Some(a) = &stamp.author_fallback {
            m.insert("author".into(), a.clone());
        }
    }
    m.insert(IMPORTED_KEY.into(), stamp.imported.clone());
    let mut tags: Vec<String> = m.get("tags").map(|t| split_tags(t)).unwrap_or_default();
    for t in &stamp.tags {
        if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            tags.push(t.clone());
        }
    }
    if !tags.is_empty() {
        m.insert("tags".into(), tags.join(", "));
    }
    m.insert(CURATED_KEY.into(), "false".into());
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The `author` name from a page's schema.org JSON-LD, when the
/// generic extractor did not surface one. Handles `@graph`, arrays of
/// authors, and `{"name": …}` vs. bare-string forms.
#[must_use]
pub fn schema_author(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(r#"script[type="application/ld+json"]"#).ok()?;
    for script in doc.select(&sel) {
        let text: String = script.text().collect();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(a) = author_in(&v) {
            return Some(a);
        }
    }
    None
}

fn author_in(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Array(items) => items.iter().find_map(author_in),
        serde_json::Value::Object(o) => {
            if let Some(g) = o.get("@graph") {
                if let Some(a) = author_in(g) {
                    return Some(a);
                }
            }
            let is_recipe = o.get("@type").is_some_and(|t| match t {
                serde_json::Value::String(s) => s.eq_ignore_ascii_case("recipe"),
                serde_json::Value::Array(ts) => ts
                    .iter()
                    .any(|t| t.as_str().is_some_and(|s| s.eq_ignore_ascii_case("recipe"))),
                _ => false,
            });
            if !is_recipe {
                return None;
            }
            author_name(o.get("author")?)
        }
        _ => None,
    }
}

fn author_name(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        serde_json::Value::Array(items) => {
            let names: Vec<String> = items.iter().filter_map(author_name).collect();
            (!names.is_empty()).then(|| names.join(", "))
        }
        serde_json::Value::Object(o) => o.get("name").and_then(author_name),
        _ => None,
    }
}

// ── Metadata rendering ───────────────────────────────────────

/// Rewrite a YAML-frontmatter cooklang source (what the synthesizers
/// emit) into the `>> key: value` metadata block the rest of the
/// cookbook is written in. Values that cannot be expressed on one
/// line (nested maps, sequences) keep the frontmatter form — the
/// source is returned unchanged rather than mangled; multi-line
/// scalars fold onto one line.
#[must_use]
pub fn frontmatter_to_arrows(source: &str) -> String {
    let Some(rest) = source.strip_prefix("---\n") else {
        return source.to_string();
    };
    let Some((yaml, body)) = rest.split_once("\n---\n") else {
        return source.to_string();
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(yaml) else {
        return source.to_string();
    };
    let mut lines = Vec::with_capacity(map.len());
    for (k, v) in &map {
        let Some(key) = k.as_str() else {
            return source.to_string();
        };
        let Some(val) = scalar_str(v) else {
            return source.to_string();
        };
        let val = val.split_whitespace().collect::<Vec<_>>().join(" ");
        if key.contains(':') || key.trim().is_empty() {
            return source.to_string();
        }
        lines.push(format!(">> {key}: {val}"));
    }
    format!("{}\n\n{}", lines.join("\n"), body.trim_start_matches('\n'))
}

fn scalar_str(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Null => Some(String::new()),
        _ => None,
    }
}

/// Read the metadata of a cooklang source in either form (`>> k: v`
/// lines or YAML frontmatter) into a flat map. Enough for the
/// resource/curated bookkeeping; the real parser stays authoritative
/// for everything the cookbook renders.
#[must_use]
pub fn metadata_of(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(rest) = source.strip_prefix("---\n") {
        if let Some((yaml, _)) = rest.split_once("\n---") {
            if let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(yaml) {
                for (k, v) in &map {
                    if let (Some(k), Some(v)) = (k.as_str(), scalar_str(v)) {
                        out.insert(k.to_string(), v);
                    }
                }
            }
        }
    }
    for line in source.lines() {
        let Some(rest) = line.strip_prefix(">>") else {
            continue;
        };
        if let Some((k, v)) = rest.split_once(':') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// `true` when the recipe's metadata marks it curated.
#[must_use]
pub fn is_curated(source: &str) -> bool {
    metadata_of(source)
        .get(CURATED_KEY)
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "yes"))
}

// ── Index page ───────────────────────────────────────────────

/// One row of the collection's index page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: String,
    /// Wiki-relative recipe path (`Cookbook/Food Wishes/x.cook`).
    pub path: String,
    pub source_url: Option<String>,
    pub curated: bool,
}

/// The `<Collection>.md` index page: `type: index`, one line per
/// imported recipe with its source link, curated ones marked. Sorted
/// by name so a regenerated page diffs cleanly.
#[must_use]
pub fn index_page(
    collection: &str,
    collection_url: Option<&str>,
    folder: &str,
    entries: &[IndexEntry],
    generated: &str,
) -> String {
    let mut rows: Vec<&IndexEntry> = entries.iter().collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let curated = rows.iter().filter(|e| e.curated).count();

    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("title: {collection}\n"));
    s.push_str("type: index\n");
    s.push_str(&format!("collection: {collection}\n"));
    s.push_str(&format!("folder: Cookbook/{folder}\n"));
    if let Some(u) = collection_url {
        s.push_str(&format!("source: {u}\n"));
    }
    s.push_str(&format!("generated: {generated}\n"));
    s.push_str("tags: [index, resource]\n");
    s.push_str("---\n\n");
    s.push_str(&format!("# {collection}\n\n"));
    s.push_str(&format!(
        "{} recipes imported as resources into `Cookbook/{folder}/`",
        rows.len()
    ));
    if let Some(u) = collection_url {
        s.push_str(&format!(" from <{u}>"));
    }
    s.push_str(&format!(
        ". {curated} curated. Regenerated by `task recipe import-collection` on {generated}; \
         hand edits here are overwritten — curate in the recipe's companion note instead.\n\n"
    ));
    for e in rows {
        let stem = e
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&e.path)
            .trim_end_matches(".cook");
        let mark = if e.curated { " ★" } else { "" };
        match &e.source_url {
            Some(u) => s.push_str(&format!("- [[{stem}|{}]]{mark} — [source]({u})\n", e.name)),
            None => s.push_str(&format!("- [[{stem}|{}]]{mark}\n", e.name)),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"<!doctype html><html><body>
<a id="mntl-card-list-items_1-0" class="comp mntl-card-list-items" data-doc-id="6650106" href="https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/">Paprika Chicken</a>
<a class="mntl-card-list-items" data-doc-id="11859335" href="https://www.allrecipes.com/amish-apple-fritter-bread-recipe-11859335">Fritter Bread</a>
<a class="mntl-card-list-items" data-doc-id="12023242" href="https://www.allrecipes.com/most-saved-chef-john-recipes-12023242">Round-up</a>
<a class="mntl-card-list-items" data-doc-id="1" href="https://www.allrecipes.com/gallery/chef-johns-best-sauces-for-grilling/">Gallery</a>
<a href="/recipe/223069/strawberry-rhubarb-custard-pie/?utm_source=x#comments">Pie (relative, tracked)</a>
<a href="https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/">dup</a>
<a href="https://www.allrecipes.com/recipes/16791/everyday-cooking/special-collections/food-wishes/">self</a>
<a href="https://www.allrecipes.com/recipes/">categories</a>
</body></html>"#;

    #[test]
    fn listing_yields_recipe_links_only_in_order_and_deduped() {
        let base = "https://www.allrecipes.com/recipes/16791/everyday-cooking/special-collections/food-wishes/";
        let links = recipe_links(LISTING, base);
        assert_eq!(
            links,
            vec![
                "https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/",
                "https://www.allrecipes.com/amish-apple-fritter-bread-recipe-11859335",
                "https://www.allrecipes.com/recipe/223069/strawberry-rhubarb-custard-pie/",
            ]
        );
    }

    #[test]
    fn enumerate_dispatches_on_shape() {
        let base = "https://www.allrecipes.com/x/";
        assert_eq!(enumerate(LISTING, base).len(), 3);
        let list = "# saved\nhttps://www.allrecipes.com/recipe/1/a/\n\nnot a url\nhttps://www.allrecipes.com/recipe/1/a/?x=1\n";
        assert_eq!(
            enumerate(list, base),
            vec!["https://www.allrecipes.com/recipe/1/a/"]
        );
    }

    #[test]
    fn recipe_url_shapes() {
        assert!(is_recipe_url(
            "https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/"
        ));
        assert!(is_recipe_url(
            "https://www.allrecipes.com/whipped-burrata-recipe-12015302"
        ));
        assert!(is_recipe_url("https://example.test/recipes/banana-bread"));
        assert!(!is_recipe_url(
            "https://www.allrecipes.com/chef-johns-best-peach-recipes-11973275"
        ));
        assert!(!is_recipe_url("https://www.allrecipes.com/recipes/"));
        assert!(!is_recipe_url(
            "https://www.allrecipes.com/recipes/16791/everyday-cooking/"
        ));
        assert!(!is_recipe_url("https://www.allrecipes.com/gallery/x/"));
    }

    #[test]
    fn canonical_url_drops_query_fragment_and_case() {
        assert_eq!(
            canonical_url("HTTPS://WWW.Allrecipes.com/recipe/1/a/?utm=x#c"),
            "https://www.allrecipes.com/recipe/1/a/"
        );
        assert_eq!(canonical_url("https://x.test"), "https://x.test/");
        assert_eq!(
            site_of("https://www.allrecipes.com/recipe/1/a/"),
            "allrecipes"
        );
        assert_eq!(
            page_file_name("https://www.allrecipes.com/recipe/1/paprika-chicken/?x"),
            "paprika-chicken.html"
        );
        assert_eq!(
            page_file_name("https://www.allrecipes.com/whipped-burrata-recipe-12015302"),
            "whipped-burrata-recipe-12015302.html"
        );
    }

    #[test]
    fn present_matches_on_canonical_source() {
        let existing = vec![
            ("Cookbook/Oatmeal.cook", None),
            (
                "Cookbook/Food Wishes/paprika-chicken.cook",
                Some(
                    "https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/?utm=1",
                ),
            ),
        ];
        let hit = find_present(
            existing.iter().map(|(p, s)| (*p, *s)),
            "https://www.allrecipes.com/recipe/221093/good-frickin-paprika-chicken/",
        );
        assert_eq!(
            hit.as_deref(),
            Some("Cookbook/Food Wishes/paprika-chicken.cook")
        );
        assert!(
            find_present(
                existing.iter().map(|(p, s)| (*p, *s)),
                "https://www.allrecipes.com/recipe/9/other/"
            )
            .is_none()
        );
    }

    fn stamp() -> ResourceStamp {
        ResourceStamp {
            collection: "Food Wishes".into(),
            site: "allrecipes".into(),
            author_fallback: Some("Chef John".into()),
            imported: "2026-09-02".into(),
            tags: vec!["resource".into(), "food-wishes".into()],
        }
    }

    #[test]
    fn stamping_sets_every_resource_key_and_merges_tags() {
        let mut r = NormalizedRecipe {
            name: "Paprika Chicken".into(),
            source_url: Some("https://www.allrecipes.com/recipe/1/a/?utm=x".into()),
            ..Default::default()
        };
        r.metadata.insert("tags".into(), "chicken, Resource".into());
        stamp_resource(&mut r, &stamp());
        assert_eq!(
            r.source_url.as_deref(),
            Some("https://www.allrecipes.com/recipe/1/a/")
        );
        assert_eq!(r.metadata["source_site"], "allrecipes");
        assert_eq!(r.metadata["collection"], "Food Wishes");
        assert_eq!(r.metadata["author"], "Chef John");
        assert_eq!(r.metadata["imported"], "2026-09-02");
        assert_eq!(r.metadata["tags"], "chicken, Resource, food-wishes");
        assert_eq!(r.metadata["curated"], "false");
    }

    #[test]
    fn page_author_wins_over_fallback() {
        let mut r = NormalizedRecipe::default();
        r.metadata.insert("author".into(), "Someone Else".into());
        stamp_resource(&mut r, &stamp());
        assert_eq!(r.metadata["author"], "Someone Else");
    }

    #[test]
    fn schema_author_reads_json_ld_shapes() {
        let html = r#"<html><head>
<script type="application/ld+json">[{"@context":"https://schema.org","@type":["Recipe","NewsArticle"],"name":"X","author":[{"@type":"Person","name":"Chef John"}]}]</script>
</head></html>"#;
        assert_eq!(schema_author(html).as_deref(), Some("Chef John"));
        let graph = r#"<script type="application/ld+json">{"@graph":[{"@type":"WebPage"},{"@type":"Recipe","author":"Jane"}]}</script>"#;
        assert_eq!(schema_author(graph).as_deref(), Some("Jane"));
        assert_eq!(schema_author("<html></html>"), None);
    }

    #[test]
    fn stamped_frontmatter_renders_as_arrows_and_parses() {
        let mut r = NormalizedRecipe {
            name: "Paprika Chicken".into(),
            description: Some("Good: frickin.".into()),
            image: Some("https://img.test/p.jpg".into()),
            ingredients: vec!["1 chicken".into(), "2 tbsp paprika".into()],
            steps: vec!["Rub the chicken with paprika.".into(), "Roast.".into()],
            source_url: Some("https://www.allrecipes.com/recipe/1/a/".into()),
            ..Default::default()
        };
        r.metadata.insert("servings".into(), "4".into());
        stamp_resource(&mut r, &stamp());
        let yaml = crate::synthesize_heuristic(&r);
        let arrows = frontmatter_to_arrows(&yaml);
        assert!(
            arrows.starts_with(">> title: Paprika Chicken\n"),
            "{arrows}"
        );
        for want in [
            ">> source: https://www.allrecipes.com/recipe/1/a/",
            ">> source_site: allrecipes",
            ">> collection: Food Wishes",
            ">> author: Chef John",
            ">> imported: 2026-09-02",
            ">> tags: resource, food-wishes",
            ">> curated: false",
            ">> servings: 4",
            ">> image: https://img.test/p.jpg",
            ">> description: Good: frickin.",
        ] {
            assert!(arrows.contains(want), "missing `{want}` in:\n{arrows}");
        }
        assert!(!arrows.contains("---"), "{arrows}");
        crate::validate_cook("x.cook", &arrows).expect("arrows form parses");

        let meta = metadata_of(&arrows);
        assert_eq!(meta["curated"], "false");
        assert_eq!(meta["description"], "Good: frickin.");
        assert!(!is_curated(&arrows));
        assert!(is_curated(">> curated: true\n\nStir."));
        assert_eq!(metadata_of(&yaml)["collection"], "Food Wishes");
    }

    #[test]
    fn arrows_keep_the_cookbook_parsers_view() {
        // The real cookbook parser reads tags + source from the arrows
        // form exactly as it would from frontmatter.
        let src = ">> title: T\n>> tags: resource, food-wishes\n>> source: https://x.test/r\n>> curated: false\n\nStir the @salt{1%pinch}.\n";
        let parser = cooklang::CooklangParser::new(
            cooklang::Extensions::all(),
            cooklang::Converter::bundled(),
        );
        let (rec, _) = parser.parse(src).into_result().expect("parses");
        let tags: Vec<String> = rec
            .metadata
            .tags()
            .map(|ts| ts.into_iter().map(|s| s.into_owned()).collect())
            .unwrap_or_default();
        assert_eq!(tags, vec!["resource", "food-wishes"]);
        assert_eq!(
            rec.metadata
                .source()
                .and_then(|s| s.url().map(str::to_string)),
            Some("https://x.test/r".to_string())
        );
    }

    #[test]
    fn frontmatter_with_nested_values_is_left_alone() {
        let src = "---\ntitle: T\nnutrition:\n  kcal: 1\n---\n\nStir.\n";
        assert_eq!(frontmatter_to_arrows(src), src);
        // Multi-line scalars fold onto one line rather than blocking.
        let folded = frontmatter_to_arrows("---\ntitle: T\nnotes: |\n  a\n  b\n---\n\nStir.\n");
        assert_eq!(folded, ">> title: T\n>> notes: a b\n\nStir.\n");
        assert_eq!(
            frontmatter_to_arrows(">> title: T\n\nStir."),
            ">> title: T\n\nStir."
        );
    }

    #[test]
    fn index_page_lists_every_recipe_sorted_with_source_links() {
        let entries = vec![
            IndexEntry {
                name: "Whipped Burrata".into(),
                path: "Cookbook/Food Wishes/whipped-burrata.cook".into(),
                source_url: Some(
                    "https://www.allrecipes.com/whipped-burrata-recipe-12015302".into(),
                ),
                curated: true,
            },
            IndexEntry {
                name: "Amish Apple Fritter Bread".into(),
                path: "Cookbook/Food Wishes/amish-apple-fritter-bread.cook".into(),
                source_url: None,
                curated: false,
            },
        ];
        let page = index_page(
            "Food Wishes",
            Some("https://www.allrecipes.com/recipes/16791/x/"),
            "Food Wishes",
            &entries,
            "2026-09-02",
        );
        assert!(
            page.starts_with("---\ntitle: Food Wishes\ntype: index\n"),
            "{page}"
        );
        let amish = page
            .find("[[amish-apple-fritter-bread|Amish Apple Fritter Bread]]")
            .unwrap();
        let burrata = page
            .find("[[whipped-burrata|Whipped Burrata]] ★ — [source](https://www.allrecipes.com/whipped-burrata-recipe-12015302)")
            .unwrap();
        assert!(amish < burrata, "{page}");
        assert!(page.contains("2 recipes imported"), "{page}");
        assert!(page.contains("1 curated"), "{page}");
    }
}
