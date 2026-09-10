# Studio Research — schema

What a page looks like in the agent's working wiki. Looser than
**Audio Production**'s, deliberately: this wiki's job is coverage, and a
contract strict enough to slow the agent down would defeat it.

## Page types

| `type:` | Lives in | What |
|---|---|---|
| `concept` | `Concepts/` | A subject the agent has read up on. The type Audio Production also declares, so a vetted one promotes across unchanged. |
| `question` | `Questions/` | Something asked and not yet settled. Audio Production declares no such type — a question is promoted only once it has become a concept or a technique, and `--type` is how the person says which. |
| `source` | `Sources/` | A summary of one imported document. |

## Required frontmatter

```yaml
title: Page title
type: concept
ai_generated: true
generated_by: <model>
sources: ["raw/sources/<file>", ...]
```

`ai_generated:` is not optional here. Every page on this wiki was
written by a model, and a page that does not say so is indistinguishable
from one an engineer wrote — which is the single distinction this wiki
exists to preserve.

## What promotion adds

A page whose vetted form has gone to another wiki gains:

```yaml
promoted_to: "audio-production::Concepts/<Page>.md"
promoted_at: 2026-09-09T12:00:00Z
```

Nothing else about it changes. The body stays exactly as the agent wrote
it, including the parts the curated version had no room for.

## Cross-references

`[[Page title]]` within this wiki; `[[slug::Page]]` into another. Bare
links here are resolved here — when a page is promoted, any bare link
the target cannot resolve is rewritten to `[[studio-research::Page]]`,
so the curated copy points back at the research rather than dangling.
