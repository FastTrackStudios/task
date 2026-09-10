//! Promotion — moving a vetted page from one wiki into another.
//!
//! # The problem this exists to solve
//!
//! An org's wikis are not peers in the way `wiki.many.set` makes them
//! sound. Some are *curated*: a person writes them, every page has been
//! read by that person, and the whole value of the thing is that its
//! contents can be trusted without re-checking. Others are *working*:
//! an agent writes them, continuously, about whatever it was asked
//! about, and their value is coverage rather than trust. The Karpathy
//! LLM-wiki pattern this feature ports is the second kind — a memory an
//! agent keeps for itself.
//!
//! Left alone those two collapse into one. Either the curated wiki
//! stays curated and the research never reaches it, or the research is
//! written straight into the curated wiki and the curation is gone —
//! not visibly, which is the worse half: nothing on the page says which
//! sentences a person read and which a model produced. The whole
//! guarantee dies quietly.
//!
//! **Promotion** is the seam. Research is written into the working
//! wiki, where it can reference the curated one (`[[bible::John.3.16]]`
//! resolves through an ordinary subscription — nothing here is needed
//! for that). When a person has read a research page and decided it is
//! right, they promote it, and the promotion is the vetting gesture:
//! deliberate, per page, recorded on both ends.
//!
//! # Why this is a copy, not a move
//!
//! The obvious implementation is a move — the page leaves the research
//! wiki and arrives in the curated one, one page, one home, no
//! duplication. That is wrong here, for two reasons.
//!
//! The first is that the two pages are not the same page. What lands in
//! the curated wiki is the vetted claim: the part a person stands
//! behind. What stays in the research wiki is the working material —
//! the dead ends, the sources that turned out to be weak, the wider
//! sweep the agent did that a curated page has no room for. Deleting
//! the second to produce the first destroys the trail that would let
//! anyone (including the agent, on its next pass) understand how the
//! vetted claim was arrived at, or notice that it was arrived at
//! badly.
//!
//! The second is that the research wiki is a working memory, and an
//! agent's next research pass reads it. A move punches a hole in that
//! memory exactly where the agent had already done its best work: the
//! subject would look *unresearched* the moment it became trusted, and
//! the agent would research it again. Promotion has to leave the
//! working wiki more complete than it found it, not less.
//!
//! So: the source page stays, and gains a `promoted_to:` line saying
//! where its vetted form went. A reader of the research page is told
//! there is a curated version; a reader of the curated page is told
//! where it came from. Neither direction has to be inferred.
//!
//! # Why a type the target does not declare is a refusal
//!
//! Every wiki's `schema.md` declares the page types it holds. The
//! research wiki and the curated wiki do not, in general, declare the
//! same ones — a research wiki full of `question` pages promoting into
//! a scripture wiki that holds `passage` pages is the normal case, not
//! an edge case.
//!
//! There are three things this code could do with a type the target has
//! never heard of. It could write the page anyway, which puts a page in
//! the curated wiki that its own schema says cannot exist — the exact
//! pollution the feature was built to prevent, arriving through the
//! feature built to prevent it. It could guess a mapping, which is
//! worse: a wrong guess is *invisible*, because the page it produces
//! looks perfectly well-formed and only its meaning is wrong. Or it can
//! refuse, name the types the target actually declares, and make the
//! caller say which one they meant with `--type`.
//!
//! It refuses. The override exists because a person really does know
//! that their `topic` page is the curated wiki's `concept` page, and
//! saying so takes four words. What is not on offer is having that
//! decided for them by a table in this file.
//!
//! # Why bare wikilinks are requalified
//!
//! A research page's body is full of `[[Other Research Page]]`. Those
//! are bare basenames, which is the convention (`wiki.link.*`) and
//! which is what makes them break on promotion: in the target wiki the
//! same text either resolves to nothing, or — much worse — resolves to
//! a *different* page that happens to share a title. A silent
//! mis-resolution is the failure mode with no symptom.
//!
//! So every bare link in the body is checked against the pages the
//! target actually holds. A link the target can resolve is left exactly
//! as written; a link it cannot is qualified to the wiki that *can*
//! resolve it — `[[Loudness War]]` becomes
//! `[[studio-research::Loudness War]]` — which is a reference into the
//! working wiki, which is where the referent genuinely lives. Links
//! that were already qualified (`[[bible::John.3.16]]`) are never
//! touched: they already say where they point.
//!
//! # What is not decided here
//!
//! Nothing in this module does I/O or knows about a server. It takes
//! the source markdown, the target's schema, and the titles the target
//! holds, and it returns the two documents to write. Every judgment the
//! promotion makes is therefore a pure function with a test, and the
//! caller ([`crate::service::Pages`] twice over, from the CLI) is only
//! plumbing.

use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};

/// Frontmatter key on a promoted page naming where it came from.
///
/// Value is a wiki-qualified path — `studio-research::Concepts/Dynamic
/// Range.md` — in the same `<wiki>::<thing>` shape every cross-wiki
/// reference in this feature uses, so a reader who can read a wikilink
/// can read this without being taught a second syntax.
pub const PROMOTED_FROM: &str = "promoted_from";

/// Frontmatter key on a source page naming where its vetted form went.
pub const PROMOTED_TO: &str = "promoted_to";

/// Frontmatter key carrying when the promotion happened, RFC3339.
///
/// Written on both ends with the same value, so "these two pages are
/// the two halves of one promotion" is checkable without a join table.
pub const PROMOTED_AT: &str = "promoted_at";

/// One page type a wiki's `schema.md` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredType {
    /// The `type:` frontmatter value — `concept`, `passage`, `topic`.
    pub name: String,
    /// The directory pages of this type live in, when the schema says.
    ///
    /// The scaffolded schema
    /// (`task wiki scaffold`) writes a three-column table that names
    /// one; the older default schema
    /// ([`crate::schema::default_schema_doc`]) writes two columns and
    /// names none. Both are legal, so this is an `Option` rather than a
    /// guess: without a declared directory the promotion keeps the
    /// source page's own directory, which is the only choice that
    /// cannot be wrong about a convention the target never stated.
    pub dir: Option<String>,
}

/// Why a promotion cannot be planned.
///
/// Every variant is a refusal the caller should print verbatim: they
/// exist because guessing past them would produce a page that looks
/// right and is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteError {
    /// The source page has no `type:` in its frontmatter, so there is
    /// nothing to translate. An untyped page in a typed wiki is already
    /// a schema violation on the source side; promoting it would move
    /// the violation somewhere it matters more.
    SourceUntyped { path: String },
    /// The target's `schema.md` declares no page types at all — either
    /// it was never written or it is prose with no type table. Without
    /// a declared set there is nothing to check a promotion against,
    /// and a check that always passes is not a check.
    TargetDeclaresNoTypes { wiki: String },
    /// The source type is not one the target declares, and no `--type`
    /// override was given.
    TypeNotDeclared {
        source_type: String,
        target_wiki: String,
        declared: Vec<String>,
    },
    /// A `--type` override naming something the target does not declare
    /// either. Refused rather than trusted: the override exists to let
    /// a person pick from the target's vocabulary, not to widen it.
    OverrideNotDeclared {
        requested: String,
        target_wiki: String,
        declared: Vec<String>,
    },
    /// Source and target resolve to the same page of the same wiki.
    /// Promoting a page onto itself would rewrite it with its own
    /// provenance and lose nothing but also mean nothing.
    SamePage { wiki: String, path: String },
}

impl std::fmt::Display for PromoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceUntyped { path } => write!(
                f,
                "`{path}` has no `type:` in its frontmatter — there is nothing to \
                 translate into the target's vocabulary. Give the source page a \
                 type first."
            ),
            Self::TargetDeclaresNoTypes { wiki } => write!(
                f,
                "`{wiki}` declares no page types in its `schema.md`, so a promotion \
                 into it cannot be checked against anything. Write the schema first \
                 (`task wiki schema write-schema --wiki {wiki} …`)."
            ),
            Self::TypeNotDeclared {
                source_type,
                target_wiki,
                declared,
            } => write!(
                f,
                "`{target_wiki}` does not declare the type `{source_type}`; it holds \
                 {}. Promoting anyway would put a page in a curated wiki that its own \
                 schema says cannot exist. Say which type you mean with \
                 `--type <one of those>`.",
                list(declared)
            ),
            Self::OverrideNotDeclared {
                requested,
                target_wiki,
                declared,
            } => write!(
                f,
                "`--type {requested}` is not a type `{target_wiki}` declares; it holds \
                 {}. The override picks from the target's vocabulary — it does not \
                 add to it.",
                list(declared)
            ),
            Self::SamePage { wiki, path } => write!(
                f,
                "source and target are both `{wiki}::{path}` — a page cannot be \
                 promoted onto itself."
            ),
        }
    }
}

impl std::error::Error for PromoteError {}

/// `a`, `b` and `c` — for an error message that names a vocabulary.
fn list(items: &[String]) -> String {
    match items {
        [] => "nothing".to_owned(),
        [one] => format!("`{one}`"),
        [rest @ .., last] => format!(
            "{} and `{last}`",
            rest.iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Everything a promotion needs to know that is not the source body.
#[derive(Debug, Clone)]
pub struct PromoteRequest<'a> {
    /// Slug of the wiki the page lives in today.
    pub from_wiki: &'a str,
    /// Wiki-root-relative path of the source page.
    pub from_path: &'a str,
    /// Slug of the wiki the vetted form is going to.
    pub to_wiki: &'a str,
    /// Explicit target path (`--as`). `None` derives one from the
    /// target type's declared directory and the source basename.
    pub to_path: Option<&'a str>,
    /// Explicit target type (`--type`). `None` requires the source's
    /// own type to be one the target declares.
    pub as_type: Option<&'a str>,
    /// The target wiki's `schema.md`, verbatim.
    pub target_schema: &'a str,
    /// Every name the target wiki can resolve a bare `[[link]]` to —
    /// page titles and file stems both, since either spelling is
    /// legal.
    pub target_names: &'a BTreeSet<String>,
    /// When the promotion happened. Injected rather than read from the
    /// clock so the rendered documents are a pure function of the
    /// inputs and can be asserted byte for byte.
    pub at: DateTime<Utc>,
}

/// The two documents a promotion produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    /// Where in the target wiki the page lands.
    pub to_path: String,
    /// The `type:` the promoted page carries.
    pub to_type: String,
    /// What to write to `to_wiki::to_path`.
    pub promoted_markdown: String,
    /// What to write back over the source page: unchanged but for the
    /// `promoted_to:` / `promoted_at:` back-reference.
    pub annotated_source: String,
    /// Bare links the target could not resolve, which were qualified to
    /// the source wiki. Reported so the caller can print them: a long
    /// list means the page leans on research the curated wiki does not
    /// have, which is worth seeing before deciding the promotion was a
    /// good idea.
    pub requalified_links: Vec<String>,
}

/// Plan a promotion, or refuse with the reason.
///
/// t[impl wiki.promote.copy] — the source page is returned annotated,
/// never deleted, and its body is copied through untouched.
///
/// t[impl wiki.promote.provenance] — both documents carry the same
/// instant, and the frontmatter the source already had (`ai_generated`
/// included) is carried across rather than dropped.
///
/// t[impl wiki.promote.schema] — the target's declared types are the
/// only vocabulary a promotion may land in; anything else refuses.
///
/// t[impl wiki.promote.links] — bare references the target cannot
/// resolve are qualified to the wiki that holds them.
///
/// Pure: same inputs, same two documents, no clock and no I/O. The
/// caller reads the source page and the target's schema and page list
/// over [`crate::service::Pages`] / [`crate::service::Schema`], calls
/// this, and — unless the caller asked for a dry run — writes the two
/// documents back over the same services.
pub fn plan(source_markdown: &str, req: &PromoteRequest<'_>) -> Result<Promotion, PromoteError> {
    let declared = declared_types(req.target_schema);
    if declared.is_empty() {
        return Err(PromoteError::TargetDeclaresNoTypes {
            wiki: req.to_wiki.to_owned(),
        });
    }
    let names: Vec<String> = declared.iter().map(|t| t.name.clone()).collect();

    let source = Frontmatter::split(source_markdown);
    let source_type = source
        .front
        .get("type")
        .map(str::to_owned)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| PromoteError::SourceUntyped {
            path: req.from_path.to_owned(),
        })?;

    // The target type: the override if one was given, otherwise the
    // source's own. Either way it must be one the target declares —
    // the override is a pick from the target's vocabulary, never an
    // addition to it (see the module docs).
    let target_type = match req.as_type.map(str::trim).filter(|t| !t.is_empty()) {
        Some(requested) => declared
            .iter()
            .find(|t| t.name == requested)
            .ok_or_else(|| PromoteError::OverrideNotDeclared {
                requested: requested.to_owned(),
                target_wiki: req.to_wiki.to_owned(),
                declared: names.clone(),
            })?,
        None => declared
            .iter()
            .find(|t| t.name == source_type)
            .ok_or_else(|| PromoteError::TypeNotDeclared {
                source_type: source_type.clone(),
                target_wiki: req.to_wiki.to_owned(),
                declared: names.clone(),
            })?,
    };

    let to_path = match req.to_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(explicit) => normalize_path(explicit),
        None => default_path(req.from_path, target_type),
    };
    if req.from_wiki == req.to_wiki && normalize_path(req.from_path) == to_path {
        return Err(PromoteError::SamePage {
            wiki: req.to_wiki.to_owned(),
            path: to_path,
        });
    }

    let at = req.at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let (body, requalified_links) = requalify(&source.body, req.from_wiki, req.target_names);

    // The promoted page. Its frontmatter is the source's, minus the
    // provenance of any *earlier* promotion (a page promoted twice
    // records the hop it actually made, not a stale one), with its type
    // translated and its origin recorded.
    //
    // `ai_generated:` and `generated_by:` are deliberately carried
    // across untouched. A person vetting a page vouches for what it
    // claims; it does not turn the prose into their own writing, and
    // `wiki.link.provenance` says borrowed knowledge stays visibly
    // borrowed. Silently dropping the flag on the way into the curated
    // wiki would launder exactly the distinction this feature exists to
    // keep.
    let mut promoted = source.front.clone();
    promoted.remove(PROMOTED_TO);
    promoted.set("type", &target_type.name);
    promoted.set(
        PROMOTED_FROM,
        &quote(&format!(
            "{}::{}",
            req.from_wiki,
            normalize_path(req.from_path)
        )),
    );
    promoted.set(PROMOTED_AT, &at);

    // The source page, kept whole — only the back-reference is added.
    // Rewriting anything else would make the promotion a destructive
    // edit of the working material, which is the thing the copy exists
    // to avoid.
    let mut annotated = source.front.clone();
    annotated.remove(PROMOTED_FROM);
    annotated.set(PROMOTED_TO, &quote(&format!("{}::{to_path}", req.to_wiki)));
    annotated.set(PROMOTED_AT, &at);

    Ok(Promotion {
        to_path,
        to_type: target_type.name.clone(),
        promoted_markdown: render(&promoted, &body),
        annotated_source: render(&annotated, &source.body),
        requalified_links,
    })
}

/// The page types a `schema.md` declares, in the order it declares
/// them.
///
/// Both schema dialects in this repo state their types as a markdown
/// table whose first column is a backticked `type:` value; the
/// scaffolded one adds a second column naming the directory. Rows are
/// read from any table in the document — a schema that lists its types
/// under a heading of its own and a schema that lists them inline both
/// work, and a table of something else contributes nothing because its
/// first cell is not a bare backticked word.
#[must_use]
pub fn declared_types(schema_md: &str) -> Vec<DeclaredType> {
    let mut out: Vec<DeclaredType> = Vec::new();
    for line in schema_md.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();
        let Some(first) = cells.first() else { continue };
        let Some(name) = backticked(first) else {
            continue;
        };
        // A header row (`| `type:` | … |`) and a separator row
        // (`|---|---|`) both fail this: a type is a bare word.
        if !is_type_name(name) {
            continue;
        }
        let dir = cells
            .get(1)
            .and_then(|c| backticked(c))
            .map(|d| d.trim_end_matches('/').to_owned())
            .filter(|d| !d.is_empty() && !d.contains(' '));
        if out.iter().any(|t| t.name == name) {
            continue;
        }
        out.push(DeclaredType {
            name: name.to_owned(),
            dir,
        });
    }
    out
}

/// `` `concept` `` → `concept`; anything else → `None`.
fn backticked(cell: &str) -> Option<&str> {
    cell.strip_prefix('`')?.strip_suffix('`')
}

/// A `type:` value is one bare lowercase word (hyphens allowed). The
/// header cell of the schema table is `` `type:` ``, which the colon
/// rules out.
fn is_type_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Where a promoted page lands when `--as` was not given.
///
/// The target type's declared directory plus the source basename. When
/// the target schema declares no directory, the source page's own
/// directory is kept: a wiki that never said where its `concept` pages
/// live has not earned a rearrangement of the caller's tree, and `--as`
/// is one flag away.
fn default_path(from_path: &str, target_type: &DeclaredType) -> String {
    let from = normalize_path(from_path);
    let base = from.rsplit('/').next().unwrap_or(&from).to_owned();
    match &target_type.dir {
        Some(dir) => format!("{dir}/{base}"),
        None => from,
    }
}

/// Collapse `./`, leading slashes and doubled separators so two
/// spellings of one path compare equal.
fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_start_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Wrap in double quotes when the value would otherwise be ambiguous
/// YAML — a `::` path is fine unquoted but reads better quoted, and a
/// value containing a colon-space genuinely needs it.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

/// Rewrite bare wikilinks the target cannot resolve into references
/// back to the wiki that can.
///
/// t[impl wiki.promote.links]
///
/// Returns the rewritten body and the link targets that were
/// qualified, in first-appearance order. Links that already name a wiki
/// (`[[bible::John.3.16]]`, `[[acme.test/music-theory::Modes@2026-09-01]]`)
/// are left untouched — they already say where they point, and
/// second-guessing them would break the one kind of link that was
/// already correct.
fn requalify(
    body: &str,
    from_wiki: &str,
    target_names: &BTreeSet<String>,
) -> (String, Vec<String>) {
    let mut out = String::with_capacity(body.len());
    let mut qualified: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("[[") {
        let (before, tail) = rest.split_at(open);
        out.push_str(before);
        let inner_start = &tail[2..];
        let Some(close) = inner_start.find("]]") else {
            // An unclosed `[[` is not a link; copy the rest verbatim
            // rather than inventing a terminator.
            out.push_str(tail);
            return (out, qualified);
        };
        let inner = &inner_start[..close];
        out.push_str("[[");
        out.push_str(&requalify_one(
            inner,
            from_wiki,
            target_names,
            &mut qualified,
        ));
        out.push_str("]]");
        rest = &inner_start[close + 2..];
    }
    out.push_str(rest);
    (out, qualified)
}

/// One link's innards: `Target#Anchor|Alias` with every part optional
/// but the target.
fn requalify_one(
    inner: &str,
    from_wiki: &str,
    target_names: &BTreeSet<String>,
    qualified: &mut Vec<String>,
) -> String {
    // Split off the alias and the anchor so only the page portion is
    // examined; both are reattached exactly as written.
    let (target, suffix) = match inner.find(['#', '|']) {
        Some(i) => (&inner[..i], &inner[i..]),
        None => (inner, ""),
    };
    let name = target.trim();
    // Already qualified — `wiki::Page`, or `org/wiki::Page@version`.
    // The reference format owns this shape; nothing here parses it.
    if name.contains("::") || name.is_empty() {
        return inner.to_owned();
    }
    if target_names.contains(name) {
        return inner.to_owned();
    }
    if !qualified.iter().any(|q| q == name) {
        qualified.push(name.to_owned());
    }
    format!("{from_wiki}::{name}{suffix}")
}

/// A page's frontmatter, kept as the lines it was written as.
///
/// Deliberately not a YAML map. A promotion is an edit to someone
/// else's document, and round-tripping it through a YAML parser would
/// reorder keys, restyle lists and drop comments — a diff full of
/// changes nobody asked for, in which the one change that matters is
/// invisible. Instead the block is held as entries (a top-level `key:`
/// line plus any indented continuation lines under it), so a nested
/// `anchors:` list survives untouched and only the keys this module
/// writes are ever rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Front {
    /// Present only when the document actually opened with `---`.
    present: bool,
    entries: Vec<(String, Vec<String>)>,
}

impl Front {
    fn parse(fm: &str) -> Self {
        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        for line in fm.lines() {
            let is_continuation = line.starts_with(' ') || line.starts_with('\t');
            if !is_continuation
                && let Some((key, _)) = line.split_once(':')
                && !key.trim().is_empty()
                && key.chars().all(|c| !c.is_whitespace())
            {
                entries.push((key.trim().to_owned(), vec![line.to_owned()]));
                continue;
            }
            match entries.last_mut() {
                // A continuation (or a blank line) belongs to the entry
                // above it, verbatim.
                Some((_, lines)) => lines.push(line.to_owned()),
                // Junk before the first key: keep it under a key no
                // caller can name, so nothing is silently dropped.
                None => entries.push((String::new(), vec![line.to_owned()])),
            }
        }
        Self {
            present: true,
            entries,
        }
    }

    /// The scalar value of a top-level key, unquoted.
    fn get(&self, key: &str) -> Option<&str> {
        let (_, lines) = self.entries.iter().find(|(k, _)| k == key)?;
        let first = lines.first()?;
        let (_, value) = first.split_once(':')?;
        Some(unquote(value.trim()))
    }

    /// Set a key in place, or append it. In place matters: rewriting a
    /// `type:` should move nothing else on the page.
    fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}: {value}");
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, lines)) => *lines = vec![line],
            None => self.entries.push((key.to_owned(), vec![line])),
        }
        self.present = true;
    }

    fn remove(&mut self, key: &str) {
        self.entries.retain(|(k, _)| k != key);
    }
}

/// A source document split into its frontmatter and its body.
struct Frontmatter {
    front: Front,
    body: String,
}

impl Frontmatter {
    /// `---\n<fm>\n---\n<body>`, or the whole thing as a body when the
    /// document has no frontmatter block.
    fn split(src: &str) -> Self {
        let normalized = src.replace("\r\n", "\n");
        let Some(rest) = normalized.strip_prefix("---\n") else {
            return Self {
                front: Front::default(),
                body: normalized,
            };
        };
        let Some(end) = rest.find("\n---\n") else {
            return Self {
                front: Front::default(),
                body: normalized,
            };
        };
        Self {
            front: Front::parse(&rest[..end]),
            body: rest[end + 5..].to_owned(),
        }
    }
}

/// Frontmatter block plus body, back into one document. A page that had
/// no frontmatter and gained keys grows a block; one that had none and
/// gained nothing keeps none.
fn render(front: &Front, body: &str) -> String {
    if !front.present || front.entries.is_empty() {
        return body.to_owned();
    }
    let mut out = String::from("---\n");
    for (_, lines) in &front.entries {
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("---\n");
    out.push_str(body);
    out
}

/// Strip one layer of matching quotes from a frontmatter scalar.
fn unquote(value: &str) -> &str {
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// A curated wiki's schema, in the three-column dialect
    /// `task wiki scaffold` writes.
    const CURATED: &str = "\
| `type:` | Lives in | What |
|---|---|---|
| `concept` | `Concepts/` | An idea. |
| `technique` | `Techniques/` | A way of doing something. |
| `source` | `Sources/` | A summary of one document. |
";

    /// The older two-column dialect `default_schema_doc` writes: types,
    /// no directories.
    const TWO_COLUMN: &str = "\
| `type:`      | What                              |
|--------------|-----------------------------------|
| `entity`     | A person, place, organization.    |
| `concept`    | An idea, technique, term.         |
";

    fn req<'a>(
        from_wiki: &'a str,
        from_path: &'a str,
        to_wiki: &'a str,
        schema: &'a str,
        target_names: &'a BTreeSet<String>,
    ) -> PromoteRequest<'a> {
        PromoteRequest {
            from_wiki,
            from_path,
            to_wiki,
            to_path: None,
            as_type: None,
            target_schema: schema,
            target_names,
            at: at(),
        }
    }

    #[test]
    fn a_three_column_schema_yields_types_with_their_directories() {
        let types = declared_types(CURATED);
        assert_eq!(
            types,
            vec![
                DeclaredType {
                    name: "concept".into(),
                    dir: Some("Concepts".into())
                },
                DeclaredType {
                    name: "technique".into(),
                    dir: Some("Techniques".into())
                },
                DeclaredType {
                    name: "source".into(),
                    dir: Some("Sources".into())
                },
            ]
        );
    }

    /// The header row's first cell is `` `type:` `` — a colon is not
    /// legal in a type name, which is what keeps the header out of the
    /// vocabulary without special-casing the word.
    #[test]
    fn the_header_row_is_not_a_page_type() {
        for t in declared_types(CURATED) {
            assert_ne!(t.name, "type:", "the header leaked into the vocabulary");
        }
        assert_eq!(declared_types(TWO_COLUMN).len(), 2);
    }

    #[test]
    fn a_two_column_schema_declares_types_and_no_directories() {
        let types = declared_types(TWO_COLUMN);
        assert_eq!(types[0].name, "entity");
        assert!(
            types.iter().all(|t| t.dir.is_none()),
            "a schema that never named a directory must not appear to have named one"
        );
    }

    /// The repo's own default schema has to parse, or every wiki
    /// bootstrapped rather than scaffolded is unpromotable-into.
    #[test]
    fn the_default_schema_declares_its_six_types() {
        let types = declared_types(crate::schema::default_schema_doc());
        let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "entity",
                "concept",
                "source",
                "synthesis",
                "comparison",
                "query"
            ]
        );
    }

    #[test]
    fn a_matching_type_lands_in_the_targets_directory_for_it() {
        let target = names(&[]);
        let plan = plan(
            "---\ntitle: Dynamic Range\ntype: concept\n---\n\n# Dynamic Range\n\nBody.\n",
            &req(
                "studio-research",
                "Concepts/Dynamic Range.md",
                "audio-production",
                CURATED,
                &target,
            ),
        )
        .expect("a declared type promotes");
        assert_eq!(plan.to_path, "Concepts/Dynamic Range.md");
        assert_eq!(plan.to_type, "concept");
    }

    /// The heart of the feature. A research wiki full of `question`
    /// pages must not be able to put a `question` into a wiki that
    /// holds concepts and techniques — not by guessing a mapping, and
    /// not by writing it anyway.
    #[test]
    fn a_type_the_target_does_not_declare_is_refused_by_name() {
        let target = names(&[]);
        let err = plan(
            "---\ntitle: Why\ntype: question\n---\n\nBody.\n",
            &req(
                "studio-research",
                "Questions/Why.md",
                "audio-production",
                CURATED,
                &target,
            ),
        )
        .expect_err("an undeclared type must refuse");
        let PromoteError::TypeNotDeclared { declared, .. } = &err else {
            panic!("wrong refusal: {err:?}");
        };
        assert_eq!(declared, &["concept", "technique", "source"]);
        let msg = err.to_string();
        assert!(msg.contains("`question`"), "{msg}");
        assert!(
            msg.contains("--type"),
            "the refusal must say the way out: {msg}"
        );
    }

    #[test]
    fn the_type_override_picks_from_the_targets_vocabulary() {
        let target = names(&[]);
        let mut r = req(
            "studio-research",
            "Questions/Why.md",
            "audio-production",
            CURATED,
            &target,
        );
        r.as_type = Some("technique");
        let plan = plan("---\ntitle: Why\ntype: question\n---\n\nBody.\n", &r)
            .expect("an override naming a declared type promotes");
        assert_eq!(plan.to_type, "technique");
        assert_eq!(plan.to_path, "Techniques/Why.md");
        assert!(
            plan.promoted_markdown.contains("type: technique"),
            "the promoted page must carry the translated type:\n{}",
            plan.promoted_markdown
        );
    }

    #[test]
    fn an_override_the_target_does_not_declare_is_refused_too() {
        let target = names(&[]);
        let mut r = req(
            "studio-research",
            "Questions/Why.md",
            "audio-production",
            CURATED,
            &target,
        );
        r.as_type = Some("passage");
        let err = plan("---\ntitle: Why\ntype: question\n---\n\nBody.\n", &r)
            .expect_err("the override widens nothing");
        assert!(
            matches!(err, PromoteError::OverrideNotDeclared { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_page_with_no_type_cannot_be_promoted() {
        let target = names(&[]);
        let err = plan(
            "---\ntitle: Loose\n---\n\nBody.\n",
            &req("a", "Loose.md", "b", CURATED, &target),
        )
        .expect_err("an untyped page refuses");
        assert!(matches!(err, PromoteError::SourceUntyped { .. }), "{err:?}");
    }

    #[test]
    fn a_target_whose_schema_declares_nothing_is_refused() {
        let target = names(&[]);
        let err = plan(
            "---\ntitle: X\ntype: concept\n---\n\nBody.\n",
            &req("a", "X.md", "b", "# Schema\n\nProse, no table.\n", &target),
        )
        .expect_err("nothing to check against is not a pass");
        assert!(
            matches!(err, PromoteError::TargetDeclaresNoTypes { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_page_cannot_be_promoted_onto_itself() {
        let target = names(&[]);
        let mut r = req("w", "Concepts/X.md", "w", CURATED, &target);
        r.to_path = Some("Concepts/X.md");
        let err = plan("---\ntitle: X\ntype: concept\n---\n\nBody.\n", &r)
            .expect_err("self-promotion is a no-op with side effects");
        assert!(matches!(err, PromoteError::SamePage { .. }), "{err:?}");
    }

    /// Provenance in both directions, with the same instant on both
    /// ends, so the two halves of one promotion can be matched up.
    #[test]
    fn both_ends_record_the_promotion() {
        let target = names(&[]);
        let plan = plan(
            "---\ntitle: Dynamic Range\ntype: concept\n---\n\nBody.\n",
            &req(
                "studio-research",
                "Concepts/Dynamic Range.md",
                "audio-production",
                CURATED,
                &target,
            ),
        )
        .expect("promotes");
        assert!(
            plan.promoted_markdown
                .contains("promoted_from: \"studio-research::Concepts/Dynamic Range.md\""),
            "{}",
            plan.promoted_markdown
        );
        assert!(
            plan.annotated_source
                .contains("promoted_to: \"audio-production::Concepts/Dynamic Range.md\""),
            "{}",
            plan.annotated_source
        );
        assert!(
            plan.promoted_markdown
                .contains("promoted_at: 2026-09-09T12:00:00Z")
        );
        assert!(
            plan.annotated_source
                .contains("promoted_at: 2026-09-09T12:00:00Z")
        );
    }

    /// The copy, not the move: the source keeps every byte of its body
    /// and every other frontmatter key it had.
    #[test]
    fn the_source_body_survives_the_promotion_untouched() {
        let target = names(&[]);
        let source = "---\ntitle: Dynamic Range\ntype: concept\ntags: [loudness, mastering]\n\
                      sources: [\"raw/sources/a.md\"]\n---\n\n# Dynamic Range\n\n\
                      A long working note.\n";
        let plan = plan(
            source,
            &req("r", "Concepts/Dynamic Range.md", "c", CURATED, &target),
        )
        .expect("promotes");
        assert!(
            plan.annotated_source
                .ends_with("# Dynamic Range\n\nA long working note.\n"),
            "the working material must be left alone:\n{}",
            plan.annotated_source
        );
        for key in [
            "tags: [loudness, mastering]",
            "sources: [\"raw/sources/a.md\"]",
        ] {
            assert!(plan.annotated_source.contains(key), "lost `{key}`");
            assert!(
                plan.promoted_markdown.contains(key),
                "lost `{key}` on the copy"
            );
        }
    }

    /// Nested YAML is the case a map-based rewrite would quietly
    /// destroy: `anchors:` is a block list, and Bible Study's pages are
    /// built on it.
    #[test]
    fn a_nested_frontmatter_block_round_trips_verbatim() {
        let target = names(&[]);
        let source = "---\ntitle: John 3\ntype: concept\nanchors:\n  - John.3.1\n  - John.3.16\n\
                      ---\n\nBody.\n";
        let plan = plan(source, &req("r", "P/John 3.md", "c", CURATED, &target)).expect("promotes");
        for doc in [&plan.promoted_markdown, &plan.annotated_source] {
            assert!(
                doc.contains("anchors:\n  - John.3.1\n  - John.3.16\n"),
                "the block list was mangled:\n{doc}"
            );
        }
    }

    /// A bare link the target can resolve stays bare; one it cannot is
    /// pointed back at the wiki that holds it; one that already names a
    /// wiki is never touched.
    #[test]
    fn bare_links_the_target_cannot_resolve_are_qualified_to_the_source_wiki() {
        let target = names(&["Equalization"]);
        let source = "---\ntitle: Dynamic Range\ntype: concept\n---\n\n\
                      See [[Equalization]], [[Loudness War]], [[Loudness War|the war]], \
                      [[Loudness War#^peak]] and [[bible::John.3.16]].\n";
        let plan = plan(
            source,
            &req(
                "studio-research",
                "Concepts/Dynamic Range.md",
                "audio-production",
                CURATED,
                &target,
            ),
        )
        .expect("promotes");
        let body = &plan.promoted_markdown;
        assert!(body.contains("[[Equalization]]"), "{body}");
        assert!(body.contains("[[studio-research::Loudness War]]"), "{body}");
        assert!(
            body.contains("[[studio-research::Loudness War|the war]]"),
            "the alias must survive: {body}"
        );
        assert!(
            body.contains("[[studio-research::Loudness War#^peak]]"),
            "the anchor must survive: {body}"
        );
        assert!(
            body.contains("[[bible::John.3.16]]"),
            "an already-qualified link must not be re-qualified: {body}"
        );
        assert_eq!(plan.requalified_links, vec!["Loudness War".to_owned()]);
    }

    /// The source's own body is not rewritten — requalification is a
    /// property of the copy, because in the source wiki the bare links
    /// were correct.
    #[test]
    fn requalification_does_not_touch_the_source() {
        let target = names(&[]);
        let source = "---\ntitle: X\ntype: concept\n---\n\nSee [[Loudness War]].\n";
        let plan = plan(source, &req("r", "X.md", "c", CURATED, &target)).expect("promotes");
        assert!(plan.annotated_source.contains("See [[Loudness War]].\n"));
    }

    /// Promoting a page that was itself promoted records the hop it
    /// made, not the one before it.
    #[test]
    fn a_second_promotion_replaces_the_earlier_provenance() {
        let target = names(&[]);
        let source = "---\ntitle: X\ntype: concept\npromoted_from: \"old::X.md\"\n\
                      promoted_at: 2020-01-01T00:00:00Z\n---\n\nBody.\n";
        let plan = plan(source, &req("mid", "X.md", "c", CURATED, &target)).expect("promotes");
        assert!(!plan.promoted_markdown.contains("old::X.md"));
        assert!(
            plan.promoted_markdown
                .contains("promoted_from: \"mid::X.md\"")
        );
        assert!(!plan.promoted_markdown.contains("2020-01-01"));
    }

    /// A two-column schema names no directory, so the page keeps the
    /// path it had rather than being filed somewhere the target never
    /// asked for.
    #[test]
    fn without_a_declared_directory_the_source_path_is_kept() {
        let target = names(&[]);
        let plan = plan(
            "---\ntitle: X\ntype: concept\n---\n\nBody.\n",
            &req("r", "Notes/X.md", "c", TWO_COLUMN, &target),
        )
        .expect("promotes");
        assert_eq!(plan.to_path, "Notes/X.md");
    }

    #[test]
    fn an_explicit_target_path_wins_and_is_normalized() {
        let target = names(&[]);
        let mut r = req("r", "Concepts/X.md", "c", CURATED, &target);
        r.to_path = Some("/Deep//Nest/./Y.md");
        let plan = plan("---\ntitle: X\ntype: concept\n---\n\nBody.\n", &r).expect("promotes");
        assert_eq!(plan.to_path, "Deep/Nest/Y.md");
    }

    /// Machine-authorship is provenance, and provenance survives
    /// vetting. A person vouching for what a page claims does not make
    /// the prose theirs.
    #[test]
    fn the_ai_generated_flag_is_carried_across_not_laundered() {
        let target = names(&[]);
        let plan = plan(
            "---\ntitle: X\ntype: concept\nai_generated: true\ngenerated_by: claude\n---\n\nBody.\n",
            &req("r", "X.md", "c", CURATED, &target),
        )
        .expect("promotes");
        assert!(plan.promoted_markdown.contains("ai_generated: true"));
        assert!(plan.promoted_markdown.contains("generated_by: claude"));
    }
}
