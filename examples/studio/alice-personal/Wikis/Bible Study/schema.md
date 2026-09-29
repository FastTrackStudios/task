# Bible Study — schema

Alice’s own Bible Study: what she has studied, in her words. A page arrives here two ways — she writes it, or she takes it from the **Bible Study Library** (`bible-study-library`) once she has studied it enough. A taken page carries `promoted_from:` and `promoted_at:`; it is hers to rewrite from then on, and it says when the library’s version has changed since. `skills/wiki-style.md` says how a page is laid out.

## Page types

The kinds of page this wiki holds — the same as the library’s, so any library page can be taken in as what it is.

| `type:` | Lives in | What |
|---|---|---|
| `passage` | `Passages/` | A chapter or pericope, as she reads it. |
| `topic` | `Topics/` | A subject she has worked through. |
| `question` | `Questions/` | A question she has answered for herself, with her reasons. |
| `person` | `People/` | A person, and why they matter to what she studies. |
| `word` | `Words/` | A Hebrew or Greek word she has studied. |
| `source` | `Sources/` | A source she has read or watched, and what she took from it. |
| `path` | `Paths/` | An order to read her own pages in. |

## Required frontmatter

```yaml
title: Page title
type: passage
summary: "The page in one sentence."
created: YYYY-MM-DD
```

## Cross-references

- A page here: `[[Page title]]`. A library page: `[[bible-study-library::Page]]` — a taken page’s links to what it did not bring along already point there.
- Scripture: `[[bible::Book.Chapter.Verse|Full Name C:V]]`.
