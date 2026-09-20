# Post Production — schema

What a page here looks like, so a subscriber on another server can tell a
delivery spec from somebody's note without opening every file.

## Page types

Every page carries a `type:` frontmatter field and lives in the directory
for its type.

| `type:` | Lives in | What |
|---|---|---|
| `concept` | `Concepts/` | An idea whoever finishes the picture has to hold: what it is, and what goes wrong without it. |
| `technique` | `Techniques/` | A way of doing something in the suite, written so someone else can repeat it. |
| `spec` | `Specs/` | A number a deliverable is measured against, with the authority it comes from. |

## Required frontmatter

```yaml
title: Page title
type: concept              # one of the table above
tags: [comma, separated]   # optional
```

## Cross-references

- A page in this wiki: `[[Page title]]` — bare basename.
- A page in another org's wiki: the qualified form,
  `[[<domain>/<wiki>::Page]]`. ACME's engineers are on another server, so
  a reference into their writing carries their domain and resolves only
  for a reader who subscribes to it — a reference addresses, it never
  authorises.
