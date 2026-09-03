//! `task wiki scaffold` — a wiki from a purpose statement.
//!
//! `wiki create` adds a wiki to the org's set and writes a `purpose.md`
//! that is a title and one paragraph; the bootstrap that follows fills
//! `schema.md` with the generic default. That is a wiki with a name
//! and no contract. This verb writes the contract: a purpose document
//! with the sections the agent reads on every ingest (what it is for,
//! who reads it, the questions it answers, what is out of scope), a
//! schema whose page types are the ones the caller named, and a
//! `Goals.md` that says what to write next — then rebuilds the catalog
//! so the three are indexed.
//!
//! Everything is rendered from the arguments by pure functions
//! ([`render_purpose`], [`render_schema`], [`render_goals`]) so the
//! shape of what lands can be pinned without a server; [`run`] is the
//! only part that talks vox, and it goes through the same Registry,
//! Schema, Pages and Catalog services every other verb uses, so the
//! org router's gates apply.
//!
//! Idempotent by construction: an existing wiki is kept, a purpose or
//! schema that someone has already filled in is kept, a `Goals.md`
//! that exists is kept. Only what is missing — or still the stub a
//! plain `create` leaves — is written, and the summary printed says
//! which.

use wiki_proto::config::{NewWiki, Visibility};

use crate::establish_for_url;

/// Page types a scaffold declares when `--types` is not given.
pub(super) const DEFAULT_TYPES: &str = "topic,question,source";

/// The type every ingest pipeline writes for a source summary
/// (`Sources/<basename>.md`); always part of a schema so the agent's
/// pages have a home.
const SOURCE_TYPE: &str = "source";

/// Everything the templates need, validated once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Blueprint {
    pub title: String,
    pub slug: String,
    pub purpose: String,
    pub visibility: Visibility,
    pub types: Vec<PageType>,
}

/// One page type the schema declares: the `type:` value and the
/// directory pages of that type live in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageType {
    /// The frontmatter value: `topic`, `question`, `person`.
    pub name: String,
    /// The directory: `Topics/`, `Questions/`, `People/`.
    pub dir: String,
}

impl Blueprint {
    /// Validate the flags into a blueprint. The slug derives from the
    /// title the way `create_wiki` derives it, so `scaffold` and
    /// `create` agree on which wiki a title names.
    pub(super) fn new(
        title: &str,
        slug: &str,
        purpose: &str,
        visibility: &str,
        types: &str,
    ) -> eyre::Result<Self> {
        let title = title.trim();
        if title.is_empty() {
            return Err(eyre::eyre!("a wiki needs a title"));
        }
        let purpose = purpose.trim();
        if purpose.is_empty() {
            return Err(eyre::eyre!(
                "a scaffold needs `--purpose <sentence>` — it is what every other file is written around"
            ));
        }
        let slug = if slug.trim().is_empty() {
            wiki_proto::config::slugify(title)
        } else {
            slug.trim().to_owned()
        };
        if slug.is_empty() || slug != wiki_proto::config::slugify(&slug) {
            return Err(eyre::eyre!(
                "`{slug}` is not a slug: lowercase words joined by single hyphens"
            ));
        }
        let visibility = Visibility::parse(visibility).ok_or_else(|| {
            eyre::eyre!("`{visibility}` is not a visibility: public, unlisted or private")
        })?;
        Ok(Self {
            title: title.to_owned(),
            slug,
            purpose: purpose.to_owned(),
            visibility,
            types: parse_types(types),
        })
    }
}

/// `topic,question,person` → the types, in order, deduplicated, with
/// `source` appended when it was not named. Empty input is the
/// default set.
pub(super) fn parse_types(spec: &str) -> Vec<PageType> {
    let spec = if spec.trim().is_empty() {
        DEFAULT_TYPES
    } else {
        spec
    };
    let mut out: Vec<PageType> = Vec::new();
    for raw in spec.split(',') {
        let name = wiki_proto::config::slugify(raw);
        if name.is_empty() || out.iter().any(|t| t.name == name) {
            continue;
        }
        out.push(PageType {
            dir: dir_for(&name),
            name,
        });
    }
    if !out.iter().any(|t| t.name == SOURCE_TYPE) {
        out.push(PageType {
            name: SOURCE_TYPE.to_owned(),
            dir: dir_for(SOURCE_TYPE),
        });
    }
    out
}

/// The directory a type's pages live in: the plural, title-cased.
fn dir_for(name: &str) -> String {
    let plural = match name {
        "person" => "people".to_owned(),
        "synthesis" => "syntheses".to_owned(),
        n if n.ends_with('y') && !n.ends_with("ay") && !n.ends_with("ey") && !n.ends_with("oy") => {
            format!("{}ies", &n[..n.len() - 1])
        }
        n if n.ends_with('s') || n.ends_with('x') || n.ends_with("ch") || n.ends_with("sh") => {
            format!("{n}es")
        }
        n => format!("{n}s"),
    };
    let mut chars = plural.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => plural,
    }
}

/// What a type is for, as the schema's table says it. The named
/// types are described; an unfamiliar one gets a line the curator
/// can sharpen.
fn describe_type(name: &str) -> String {
    match name {
        "topic" => "One subject the wiki covers, synthesised across its sources.".into(),
        "question" => {
            "A question the wiki set out to answer: the answer, and the citations behind it.".into()
        }
        "person" => "A person: who they are, and what they said or did that matters here.".into(),
        "source" => "A summary of one imported document under `raw/sources/`.".into(),
        "entity" => "A place, organization, product or project.".into(),
        "concept" => "An idea, technique, term or pattern.".into(),
        "synthesis" => "A cross-cutting view written across several pages.".into(),
        "comparison" => "A side-by-side of two or more pages.".into(),
        "query" => "A filed answer to a question, with citations.".into(),
        other => format!("A `{other}` page. (Curator: say what belongs here.)"),
    }
}

/// `purpose.md`: the sentence the caller gave, and the four sections
/// the agent reads on every ingest, written around it.
pub(super) fn render_purpose(bp: &Blueprint) -> String {
    let who = match bp.visibility {
        Visibility::Public => {
            "Anyone: the wiki is public, listed in discovery, and any org may subscribe to it."
        }
        Visibility::Unlisted => {
            "Whoever holds the reference: the wiki is unlisted — not advertised, but any org given its slug may subscribe."
        }
        Visibility::Private => {
            "Members of the owning org only: the wiki is private, and a subscription from outside is refused."
        }
    };
    let mut questions = String::new();
    for t in &bp.types {
        let line = match t.name.as_str() {
            "topic" => "What does this wiki hold about a subject? — one `Topics/` page per subject, synthesised across sources.".to_owned(),
            "question" => "What has been asked here, and what was the answer? — `Questions/`, each with its citations.".to_owned(),
            "person" => "Who is this person, and why do they matter here? — `People/`.".to_owned(),
            "source" => "Where did a claim come from? — every page carries `sources:`, and `Sources/` summarises each imported document.".to_owned(),
            other => format!("What `{other}` pages does it hold? — `{}/`.", t.dir),
        };
        questions.push_str("- ");
        questions.push_str(&line);
        questions.push('\n');
    }
    format!(
        "---\n\
         title: \"{title}\"\n\
         type: purpose\n\
         ---\n\
         \n\
         # {title}\n\
         \n\
         {purpose}\n\
         \n\
         ## What this wiki is for\n\
         \n\
         {purpose} Everything in it serves that sentence; a page that does not \
         belongs somewhere else — the vault for personal notes, another wiki for \
         another subject.\n\
         \n\
         ## Who reads it\n\
         \n\
         {who} Its Editors write it; anyone who resolves a `{slug}::Page` \
         reference from their own vault reads it.\n\
         \n\
         ## Questions it answers\n\
         \n\
         {questions}\
         \n\
         ## Out of scope\n\
         \n\
         - Personal notes, tasks and journals — those belong in the vault, not a \
         published wiki.\n\
         - Anything the purpose above does not cover. Propose a change to this \
         document before widening the wiki.\n\
         \n\
         ## How it grows\n\
         \n\
         Sources are ingested (`task wiki ingest --wiki {slug} --source <file>`), \
         the agent's proposals wait in the review queue (`task wiki review list \
         --wiki {slug}`), lint findings are resolved, and gaps become research \
         plans. `schema.md` says what a page looks like; `Goals.md` says what to \
         write next.\n",
        title = bp.title,
        purpose = bp.purpose,
        slug = bp.slug,
    )
}

/// `schema.md`: the page types the caller named, each with its
/// directory, plus the frontmatter, wikilink and catalog rules every
/// wiki shares (the ones `default_schema_doc` carries).
pub(super) fn render_schema(bp: &Blueprint) -> String {
    let mut rows = String::new();
    for t in &bp.types {
        rows.push_str(&format!(
            "| `{}` | `{}/` | {} |\n",
            t.name,
            t.dir,
            describe_type(&t.name)
        ));
    }
    let first = bp.types.first().map_or("topic", |t| t.name.as_str());
    format!(
        "# {title} — schema\n\
         \n\
         The contract between the curator (human) and the maintainer (LLM agent) \
         for the `{slug}` wiki. The agent reads this on every ingest.\n\
         \n\
         ## Page types\n\
         \n\
         Every page carries a `type:` frontmatter field and lives in the \
         directory for its type.\n\
         \n\
         | `type:` | Lives in | What |\n\
         |---|---|---|\n\
         {rows}\
         \n\
         Pages outside this wiki use other types (`task`, `daily`, `meeting`, …) — \
         those are not wiki pages.\n\
         \n\
         ## Required frontmatter\n\
         \n\
         ```yaml\n\
         title: Page title\n\
         type: {first}              # one of the table above\n\
         tags: [comma, separated]   # optional but recommended\n\
         sources: [\"raw/sources/<file>\", ...]  # required for source pages, and for any claim taken from one\n\
         created: YYYY-MM-DD\n\
         ```\n\
         \n\
         Pages the agent writes also carry `ai_generated: true` and `generated_by: <model>`.\n\
         \n\
         ## Cross-references\n\
         \n\
         - A page in this wiki: `[[Page title]]` — bare basename, so folder moves \
         don't break links.\n\
         - A page in a wiki this one subscribes to: `[[slug::Page]]`, resolved \
         through the subscription rather than copied here. Scripture is \
         `[[bible::Book.Chapter.Verse]]` — `[[bible::John.3.16]]`.\n\
         - Never link out to the vault; the vault links in.\n\
         \n\
         ## Catalog + log\n\
         \n\
         - `index.md` is the catalog, organised by `type:`. The agent updates it on \
         every ingest; `task wiki catalog rebuild --wiki {slug}` rebuilds it from the tree.\n\
         - `log.md` is append-only. Each entry starts `## [YYYY-MM-DD] <op> | <title>` \
         so `grep '^## \\['` gives a clean timeline.\n\
         - `purpose.md` says what belongs here; `Goals.md` says what to write next.\n",
        title = bp.title,
        slug = bp.slug,
    )
}

/// `Goals.md` when no `--goals-file` was given: a starter list that
/// points at the first page of each type.
pub(super) fn render_goals(bp: &Blueprint) -> String {
    let mut items = String::new();
    for t in &bp.types {
        let line = match t.name.as_str() {
            "source" => {
                "Ingest the first source and work through what the agent proposes.".to_owned()
            }
            "question" => "Ask the first question and answer it from sources.".to_owned(),
            other => format!(
                "Write the first `{dir}/` page — the {other} the purpose names first.",
                dir = t.dir
            ),
        };
        items.push_str("- [ ] ");
        items.push_str(&line);
        items.push('\n');
    }
    format!(
        "---\n\
         title: \"Goals\"\n\
         type: goals\n\
         ---\n\
         \n\
         # Goals — {title}\n\
         \n\
         What to write next, in order. Tick a goal when a page (or a set of pages) \
         answers it; add new ones as reading raises them. The agent reads this \
         beside `purpose.md`.\n\
         \n\
         {items}",
        title = bp.title,
    )
}

/// Whether a `purpose.md` is still what `create`/bootstrap leave: the
/// default stub, or a title with at most one paragraph and no
/// sections. A purpose someone has written has `## ` headings.
pub(super) fn is_stub_purpose(markdown: &str) -> bool {
    let text = markdown.trim();
    text.is_empty()
        || text == wiki_proto::schema::default_purpose_doc().trim()
        || text.contains("Curator: fill this in")
        || !text.lines().any(|l| l.starts_with("## "))
}

/// Whether a `schema.md` is still the generic default.
pub(super) fn is_stub_schema(markdown: &str) -> bool {
    let text = markdown.trim();
    text.is_empty() || text == wiki_proto::schema::default_schema_doc().trim()
}

/// The path `Goals.md` is written to, relative to the wiki root.
const GOALS_MD: &str = "Goals.md";

/// Create-if-missing, then fill what is missing, over vox.
pub(super) async fn run(url: &str, bp: &Blueprint, goals: Option<&str>) -> eyre::Result<()> {
    use wiki_proto::service::catalog::CatalogClient;
    use wiki_proto::service::pages::PagesClient;
    use wiki_proto::service::registry::RegistryClient;
    use wiki_proto::service::schema::SchemaClient;

    let registry: RegistryClient = establish_for_url(url).await?;
    let existing = registry
        .list_wikis()
        .await
        .map_err(|e| eyre::eyre!("list wikis: {e:?}"))?
        .into_iter()
        .find(|w| w.slug == bp.slug);
    match existing {
        Some(w) => println!("wiki    `{}` exists ({})", w.slug, w.title),
        None => {
            let w = registry
                .create_wiki(NewWiki {
                    title: bp.title.clone(),
                    slug: bp.slug.clone(),
                    purpose: bp.purpose.clone(),
                    visibility: bp.visibility,
                    source: None,
                })
                .await
                .map_err(|e| eyre::eyre!("create wiki `{}`: {e:?}", bp.slug))?;
            println!(
                "wiki    created `{}` ({}, {})",
                w.slug,
                w.title,
                w.visibility.as_str()
            );
        }
    }

    let schema: SchemaClient = establish_for_url(url).await?;
    let purpose_now = schema.read_purpose(bp.slug.clone()).await.ok();
    if purpose_now.is_none_or(|d| is_stub_purpose(&d.markdown)) {
        schema
            .write_purpose(bp.slug.clone(), render_purpose(bp))
            .await
            .map_err(|e| eyre::eyre!("write purpose.md: {e:?}"))?;
        println!("wrote   purpose.md");
    } else {
        println!("kept    purpose.md (already written)");
    }
    let schema_now = schema.read_schema(bp.slug.clone()).await.ok();
    if schema_now.is_none_or(|d| is_stub_schema(&d.markdown)) {
        schema
            .write_schema(bp.slug.clone(), render_schema(bp))
            .await
            .map_err(|e| eyre::eyre!("write schema.md: {e:?}"))?;
        println!(
            "wrote   schema.md ({})",
            bp.types
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!("kept    schema.md (already written)");
    }

    let pages: PagesClient = establish_for_url(url).await?;
    if pages
        .read_page(bp.slug.clone(), GOALS_MD.to_owned())
        .await
        .is_ok()
    {
        println!("kept    {GOALS_MD} (exists)");
    } else {
        let body = goals.map_or_else(|| render_goals(bp), str::to_owned);
        pages
            .write_page(bp.slug.clone(), GOALS_MD.to_owned(), body, String::new())
            .await
            .map_err(|e| eyre::eyre!("write {GOALS_MD}: {e:?}"))?;
        println!(
            "wrote   {GOALS_MD}{}",
            if goals.is_some() {
                " (from --goals-file)"
            } else {
                ""
            }
        );
    }

    let catalog: CatalogClient = establish_for_url(url).await?;
    catalog
        .rebuild_index(bp.slug.clone())
        .await
        .map_err(|e| eyre::eyre!("rebuild catalog: {e:?}"))?;
    println!("rebuilt index.md");
    println!(
        "\nnext:   task wiki page list {slug}   ·   task wiki ingest --wiki {slug} --source <file>",
        slug = bp.slug
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bible() -> Blueprint {
        Blueprint::new(
            "Bible Study",
            "",
            "Notes and questions from a weekly study of the Gospel of John.",
            "unlisted",
            "topic,question,person",
        )
        .unwrap()
    }

    #[test]
    fn the_slug_derives_from_the_title_the_way_create_does() {
        let bp = bible();
        assert_eq!(bp.slug, wiki_proto::config::slugify("Bible Study"));
        assert_eq!(bp.slug, "bible-study");
        assert_eq!(bp.visibility, Visibility::Unlisted);
        assert!(Blueprint::new("Bible Study", "Not A Slug", "p", "private", "").is_err());
        assert!(Blueprint::new("", "", "p", "private", "").is_err());
        assert!(Blueprint::new("T", "", "   ", "private", "").is_err());
        assert!(Blueprint::new("T", "", "p", "secret", "").is_err());
    }

    #[test]
    fn types_are_deduplicated_pluralised_and_always_include_source() {
        let types = parse_types("topic, Question,person,topic");
        let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["topic", "question", "person", "source"]);
        let dirs: Vec<&str> = types.iter().map(|t| t.dir.as_str()).collect();
        assert_eq!(dirs, ["Topics", "Questions", "People", "Sources"]);

        let default: Vec<String> = parse_types("").into_iter().map(|t| t.name).collect();
        assert_eq!(default, ["topic", "question", "source"]);

        // `source` named explicitly is not doubled, and a type the
        // schema already knows keeps its conventional directory.
        let names: Vec<String> = parse_types("source,concept,entity,synthesis,query")
            .into_iter()
            .map(|t| t.dir)
            .collect();
        assert_eq!(
            names,
            ["Sources", "Concepts", "Entities", "Syntheses", "Queries"]
        );
    }

    #[test]
    fn purpose_is_a_document_written_around_the_sentence() {
        let bp = bible();
        let doc = render_purpose(&bp);
        assert!(doc.starts_with("---\ntitle: \"Bible Study\"\n"), "{doc}");
        assert!(doc.contains("# Bible Study\n"), "{doc}");
        assert!(doc.contains(&bp.purpose), "{doc}");
        for section in [
            "## What this wiki is for",
            "## Who reads it",
            "## Questions it answers",
            "## Out of scope",
        ] {
            assert!(doc.contains(section), "missing {section}: {doc}");
        }
        // Visibility is explained, not just named.
        assert!(doc.contains("unlisted"), "{doc}");
        // Each type contributes a question the wiki answers.
        assert!(doc.contains("`Topics/`"), "{doc}");
        assert!(doc.contains("`Questions/`"), "{doc}");
        assert!(doc.contains("`People/`"), "{doc}");
        assert!(doc.contains("`Sources/`"), "{doc}");
        // The verbs it names address this wiki, not "default".
        assert!(doc.contains("--wiki bible-study"), "{doc}");
        assert!(!doc.contains("default"), "{doc}");
        // And it is no longer a stub.
        assert!(!is_stub_purpose(&doc));
    }

    #[test]
    fn schema_declares_the_named_types_and_keeps_the_shared_rules() {
        let bp = bible();
        let doc = render_schema(&bp);
        assert!(doc.starts_with("# Bible Study — schema\n"), "{doc}");
        assert!(doc.contains("| `topic` | `Topics/` |"), "{doc}");
        assert!(doc.contains("| `question` | `Questions/` |"), "{doc}");
        assert!(doc.contains("| `person` | `People/` |"), "{doc}");
        assert!(doc.contains("| `source` | `Sources/` |"), "{doc}");
        assert!(doc.contains("type: topic"), "{doc}");
        // The rules `default_schema_doc` carries survive: frontmatter,
        // wikilinks, catalog and log.
        for rule in [
            "## Required frontmatter",
            "sources:",
            "`[[Page title]]`",
            "`[[slug::Page]]`",
            "`[[bible::John.3.16]]`",
            "`index.md` is the catalog",
            "`log.md` is append-only",
            "## [YYYY-MM-DD] <op> | <title>",
        ] {
            assert!(doc.contains(rule), "missing {rule}: {doc}");
        }
        assert!(doc.contains("catalog rebuild --wiki bible-study"), "{doc}");
        assert!(!is_stub_schema(&doc));
    }

    #[test]
    fn goals_stub_points_at_the_first_page_of_each_type() {
        let doc = render_goals(&bible());
        assert!(doc.contains("# Goals — Bible Study"), "{doc}");
        assert!(
            doc.contains("- [ ] Write the first `Topics/` page"),
            "{doc}"
        );
        assert!(
            doc.contains("- [ ] Write the first `People/` page"),
            "{doc}"
        );
        assert!(doc.contains("- [ ] Ask the first question"), "{doc}");
        assert!(doc.contains("- [ ] Ingest the first source"), "{doc}");
    }

    /// What `create` and the bootstrap leave is a stub; what a person
    /// or this verb writes is not.
    #[test]
    fn stubs_are_told_from_written_documents() {
        assert!(is_stub_purpose(""));
        assert!(is_stub_purpose(wiki_proto::schema::default_purpose_doc()));
        // `create_wiki` with a purpose sentence: title + paragraph.
        assert!(is_stub_purpose(
            "---\ntitle: \"Bible Study\"\n---\n\n# Bible Study\n\nA sentence.\n"
        ));
        assert!(!is_stub_purpose(
            "# Mine\n\n## What this wiki is for\n\nWritten by hand.\n"
        ));
        assert!(is_stub_schema(""));
        assert!(is_stub_schema(wiki_proto::schema::default_schema_doc()));
        assert!(!is_stub_schema("# Mine — schema\n\n| `x` | `Xs/` | y |\n"));
    }
}
