# wiki-needs-research-lint — surface `needs_research` markers as lint findings

**Status:** not started. Nothing in the tree reads `needs_research` — a repo-wide grep over `features/task/wiki/**/*.rs` returns zero hits.

**Target:** make the `needs_research:` frontmatter array and the `> [!research]` inline callout first-class signals in the wiki lint loop, so an agent (or the human curator) can drain them via the existing `task wiki lint-findings` queue.

Today the convention exists in pages (see `Knowledge/wiki/concepts/needs-research-tag.md`) but no code scans for it. The markers are greppable but invisible to the standard maintenance loop.

## What we have today

`features/wiki/wiki-lint/` already produces `LintFinding`s of several kinds — `MissingPage`, `Orphan`, `BrokenWikilink`, `MissingSource`, `Duplicate`, etc. — and stores them in `<vault>/_state/lint_findings.json`. The `task wiki lint-findings` CLI walks that file and lets the curator `resolve / dismiss / promote-review / promote-research`.

The scanner sits in `wiki-lint::scan::scan_vault(root)` and dispatches to one function per finding kind. Adding `NeedsResearch` is mechanically the same shape as the existing kinds.

## Design

### New finding kind

```rust
pub enum LintFindingKind {
    // … existing variants …
    NeedsResearch,
}

pub struct NeedsResearchFinding {
    pub page: PathBuf,
    pub origin: NeedsResearchOrigin,   // Frontmatter | InlineCallout { line: usize }
    pub prompt: String,                // the actual question / gap statement
}

pub enum NeedsResearchOrigin {
    Frontmatter,
    InlineCallout { line: usize },
}
```

### Scanner

Two passes, both per-page:

1. **Frontmatter pass.** Parse the YAML; if `needs_research` is present and is a non-empty sequence, emit one finding per array entry.

2. **Body pass.** Regex over lines for `^>\s+\[!research\]\s+(.+)$`. The matched group is the prompt. Subsequent `^>\s+(.+)$` lines extend the prompt until a non-quote line.

```rust
fn scan_needs_research(page: &Page, src: &str) -> Vec<NeedsResearchFinding> {
    let mut out = Vec::new();

    // 1. Frontmatter array.
    if let Some(prompts) = page.frontmatter.get("needs_research").and_then(|v| v.as_sequence()) {
        for p in prompts.iter().filter_map(|v| v.as_str()) {
            out.push(NeedsResearchFinding {
                page: page.path.clone(),
                origin: NeedsResearchOrigin::Frontmatter,
                prompt: p.to_string(),
            });
        }
    }

    // 2. Inline `> [!research]` callouts.
    let re = regex::Regex::new(r"(?m)^>\s+\[!research\]\s+(.+)$").unwrap();
    for cap in re.captures_iter(src) {
        let m = cap.get(0).unwrap();
        let line = src[..m.start()].matches('\n').count() + 1;
        out.push(NeedsResearchFinding {
            page: page.path.clone(),
            origin: NeedsResearchOrigin::InlineCallout { line },
            prompt: cap[1].to_string(),
        });
    }

    out
}
```

### CLI surface

The existing `task wiki lint-findings list / resolve` already covers display + resolution. New finding kind needs:

- Pretty-printer for `NeedsResearch` (page path, origin, prompt).
- `promote-research` already works — it converts any finding to a `ResearchPlan`. For `NeedsResearch` findings, the plan's `gap_kind` defaults to `NeedsResearch` and the `gap_description` is the prompt.

Optional convenience:

```bash
task wiki needs-research list --org <slug>            # filtered view
task wiki needs-research drain --org <slug>           # promote all to research plans
```

Implemented as thin wrappers over `lint-findings list --kind needs-research` / `lint-findings resolve <id> promote-research`.

### Removal semantics

When the curator resolves a `NeedsResearch` finding as `Resolved` (the research was completed and the page updated), the scanner needs to **not** re-emit the same finding on the next scan. Two strategies:

1. **Author removes the tag** when they update the page. Simple; relies on discipline.
2. **Lint state stores resolved IDs** keyed by `(page, prompt)` hash. Survives re-scans. The existing `_state/lint_findings.json` already has a `resolved_ids: HashSet<FindingId>` field — extend it.

Pick #1 first (it's idempotent + greppable); add #2 only if dropping-the-tag-then-forgetting becomes a real source of noise.

## Acceptance criteria

- [ ] `wiki-lint::scan::scan_vault` returns `NeedsResearch` findings for every frontmatter `needs_research: [...]` entry and every `> [!research] ...` body callout.
- [ ] `task wiki lint-findings list` displays `NeedsResearch` findings with page + line + prompt.
- [ ] `task wiki lint-findings resolve <id> promote-research` creates a `ResearchPlan` with the prompt as `gap_description`.
- [ ] `task wiki needs-research list / drain` convenience wrappers work.
- [ ] Existing tests still pass; new tests cover both origin types + the resolved-ID short-circuit.
- [ ] `Knowledge/wiki/concepts/wiki-linting.md` mentions the new kind.

## Out of scope

- LLM-driven research execution. Drain → research plan; the plan is *executed* through the existing `task wiki research` workflow (web search, source upload, re-ingest). Don't try to auto-resolve from inside the lint loop.
- Cross-page `needs_research` deduplication. Two pages each independently requesting the same research → two findings. The curator promotes one and dismisses the other manually.
- A "research-completed" auto-detection (page was updated since the tag was added → maybe resolved?). Too heuristic; leave to the curator.

## File-level breakdown

| File | Change |
|---|---|
| `features/wiki/wiki-lint/src/finding.rs` | Add `NeedsResearch` variant + structured payload |
| `features/wiki/wiki-lint/src/scan.rs` | New `scan_needs_research` function; wire into `scan_vault` |
| `features/wiki/wiki-lint/src/display.rs` | Pretty-printer arm for `NeedsResearch` |
| `features/wiki/wiki-lint/src/promote.rs` | Map `NeedsResearch` → `ResearchPlan` (`gap_kind` + `gap_description`) |
| `apps/cli/src/main.rs` | `task wiki needs-research list / drain` subcommands |
| `features/wiki/wiki-lint/tests/needs_research.rs` | New integration tests (frontmatter, callout, mixed) |
| `Knowledge/wiki/concepts/wiki-linting.md` | Add the new finding kind to the kinds list |

## Effort estimate

Half a day. Pattern matches the existing finding kinds; no new infrastructure needed.

## Related

- `Knowledge/wiki/concepts/needs-research-tag.md` — the user-facing convention this plan implements support for
- `Knowledge/wiki/concepts/wiki-linting.md` — the broader lint surface
- `plans/wiki-feature.md` — parent epic
