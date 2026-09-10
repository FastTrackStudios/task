# Audio Production — schema

The contract between the curator (a human engineer) and anyone — person
or agent — proposing a change. Everything that lands here has been read
by an engineer; that is the only property this wiki has that a search
engine does not, and the schema is how it is kept.

## Page types

Every page carries a `type:` frontmatter field and lives in the
directory for its type.

| `type:` | Lives in | What |
|---|---|---|
| `concept` | `Concepts/` | An idea an engineer needs to hold: what it is, and what goes wrong without it. |
| `technique` | `Techniques/` | A way of doing something in the room, written so someone else can repeat it. |
| `source` | `Sources/` | A summary of one imported document, with its provenance. |

There is no `question` type here on purpose. An open question is
research, and research belongs in **Studio Research** until it has an
answer an engineer will stand behind — at which point it is a concept or
a technique, and it arrives by `task wiki promote`.

## Required frontmatter

```yaml
title: Page title
type: concept              # one of the table above
tags: [comma, separated]   # optional
sources: ["raw/sources/<file>", ...]  # for any claim taken from one
```

A page that arrived by promotion also carries:

```yaml
promoted_from: "studio-research::Concepts/<Page>.md"
promoted_at: 2026-09-09T12:00:00Z
```

and keeps `ai_generated: true` / `generated_by:` if the research draft
had them. Vetting says an engineer vouches for what the page claims; it
does not turn a model's prose into an engineer's writing.

## Cross-references

- A page in this wiki: `[[Page title]]` — bare basename.
- A page in another wiki: `[[slug::Page]]`.
- A bare link that this wiki cannot resolve is a bug. Promotion
  qualifies unresolvable links back to the wiki that holds them rather
  than leaving them to dangle or, worse, to land on a same-named page
  here that means something else.
