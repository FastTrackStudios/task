//! Schema + purpose documents — the per-vault contract.
//!
//! `<vault>/Wiki/schema.md` defines structural conventions (page
//! types, frontmatter keys, naming, what goes in `index.md` and
//! `log.md`). `<vault>/Wiki/purpose.md` describes the wiki's
//! goals — what's being accumulated, why, who reads it, what
//! questions it should be able to answer.
//!
//! Both are plain markdown; backends read + write them
//! verbatim. The LLM agent loads them on every ingest as
//! context, so updates take effect on the next call.

use facet::Facet;

/// `<vault>/Wiki/schema.md` content.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SchemaDoc {
    /// Raw markdown body, frontmatter intact.
    pub markdown: String,
    /// Last-modified timestamp (RFC3339), if the backend
    /// tracks it. Empty string if unknown.
    pub modified: String,
}

/// `<vault>/Wiki/purpose.md` content.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct PurposeDoc {
    /// Raw markdown body, frontmatter intact.
    pub markdown: String,
    /// Last-modified timestamp (RFC3339), if the backend
    /// tracks it.
    pub modified: String,
}

/// Default `schema.md` text written when a wiki is first
/// bootstrapped. Captures the conventions this crate assumes:
/// page types, frontmatter keys, where the catalog + log live.
/// Backends are free to override; the LLM agent reads whatever
/// the file actually contains.
#[must_use]
pub fn default_schema_doc() -> &'static str {
    DEFAULT_SCHEMA
}

/// Default `purpose.md` text — a stub for the curator to fill
/// in. Tells the agent: "this is undefined, ask the curator
/// before writing anything substantial."
#[must_use]
pub fn default_purpose_doc() -> &'static str {
    DEFAULT_PURPOSE
}

const DEFAULT_SCHEMA: &str = r#"# Wiki schema

The contract between the curator (human) and the maintainer (LLM agent).

## Page types

Every wiki page carries a `type:` frontmatter field. Recognized values:

| `type:`      | What                                              |
|--------------|---------------------------------------------------|
| `entity`     | A person, place, organization, product, project. |
| `concept`    | An idea, technique, term, pattern.               |
| `source`     | A summary of a single imported document.         |
| `synthesis`  | An LLM- or human-authored cross-cutting view.    |
| `comparison` | A side-by-side of two or more entities/concepts. |
| `query`      | A filed answer to a question, with citations.    |

Pages outside `Wiki/` use other types (`task`, `daily`, `meeting`, etc.) — those are not wiki pages.

## Required frontmatter

```yaml
title: Page title
type: concept            # see table above
tags: [comma, separated] # optional but recommended
sources: ["src-id", ...] # required for source/synthesis/comparison/query
created: 2026-05-21
```

## Cross-references

Use `[[Page title]]` wikilinks. Bare basename; folder moves don't break links.

## Catalog + log

- `Wiki/index.md` is the catalog. Organized by `type:`. The LLM updates it on every ingest.
- `Wiki/log.md` is append-only. Each entry starts `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \['` gives a clean timeline.
"#;

const DEFAULT_PURPOSE: &str = r"# Wiki purpose

> Curator: fill this in. The agent reads it on every ingest to know what to emphasize.

## What this wiki is for

(One paragraph: what knowledge is being accumulated, and why.)

## Who reads it

(You? Teammates? Future-you in 6 months?)

## Key questions it should answer

- ...
- ...

## Out of scope

(Anything explicitly *not* in this wiki's remit.)
";
