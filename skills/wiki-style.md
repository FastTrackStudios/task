---
name: wiki-style
description: Write or restyle a Task wiki page so a hard subject reads fast and every page looks like its neighbours — the page anatomy per type, which rendered element to reach for when (summary lede, callouts, verse cards, readings, timelines, maps, tabs, word/source/scripture badges, section embeds, study paths), the exact syntax, the house rules, and a check before you save. Use whenever an agent writes, rewrites, reviews or bulk-formats wiki pages.
---

# Wiki style — pages that read fast and look alike

A wiki page is read far more often than it is written, usually by someone
who wants one answer and may never scroll. Task's editor renders a lot of
structure from plain markdown; this skill says **which structure carries
which kind of information**, so the reader always finds the same thing in
the same place, and so every page on a subject looks like its neighbours.

Pairs with [`llm-wiki.md`](llm-wiki.md) (how to create, write and upload
pages). This skill is about what goes *inside* a page.

## The five rules

1. **The answer first.** `summary:` says the page in one sentence; the
   first paragraph gives the answer or the claim. Evidence comes after.
2. **One job per element.** Each rendered element below has one kind of
   content it is for. Don't use a callout as decoration or a table as
   layout.
3. **Depth folds.** Anything a first-time reader can skip — how certain
   it is, other readings, method, long lists — goes in a folded callout.
   The page reads as its argument; the doubt is one click away.
4. **Link, don't copy.** If another page already says it, link it or embed
   its section (`![[Page#Heading]]`). One place holds a thing.
5. **Every claim can be checked.** A claim from a source carries a
   citation badge to the exact place (`[[source#^t1226|20:26]]`); a
   verse carries its reference badge.

## Page anatomy

Every page:

```markdown
---
title: Psalm 82
type: passage
summary: "A trial in heaven: God judges the gods who misruled the nations."
tags: [divine-council, psalms]
sources: ["raw/sources/<source>.md"]
created: 2026-09-28
---

# Psalm 82

<the answer or claim, 1–3 sentences>

## <sections, each one idea>

> [!question]- How firm is this?
> <certainty, dissent, method — folded>
```

- `summary:` — one sentence, ≤ 140 characters, plain words (it becomes the
  lede under the title *and* the hover card on every link to the page).
  No links in it; emphasis only if it matters.
- The H1 is the title, once. Nothing between the frontmatter and it.
- `##` sections, `###` at most beneath. A section heading names its idea
  (“Who are the gods?”), not its format (“Table”).
- End with the folded doubt, then `See also` if needed. Never end on a
  table or a list with no closing sentence.

### By type

| `type:` | Opens with | Then | Signature element |
|---|---|---|---|
| `question` | `## The short answer` — 2–4 sentences | the case, claim by claim | a ```` ```readings ```` block for the contested point |
| `topic` | the thesis in one paragraph | sections, one idea each | verse cards for its key texts; a timeline if it has a history |
| `passage` | what the passage is, in a sentence | walk-through, readings, where it leads | a table of verses (reference badge + what happens) |
| `person` | who, and why they matter here | what they said/did that matters | citations to where they appear |
| `word` | the gloss, in italics | where it is used | frontmatter `lemma`, `translit`, `strongs`, `language`, `gloss` |
| `source` | author · length · what it is | `## Overview`, `## Sections` (each with its start badge), `## Notes` | timestamped notes `- [mm:ss] … ^t<sec>-noteN` |
| `path` | who the path is for | a numbered list, `1. [[Page]] — why it is here` | nothing else: the app draws the progress and footers |

## Which element, when

Reach for these in this order of preference: prose → list → the element.
An element earns its place when the reader would otherwise have to build
the structure in their head.

### Badges — inline, never spread a line

| You are writing | Write | Renders as |
|---|---|---|
| A Bible reference | `[[bible::Ps.82.1\|Psalm 82:1]]` | brown badge, book · **chapter** · verses, verse on hover |
| A range | `[[bible::Ps.82.2-Ps.82.4\|Psalm 82:2–4]]` | en dash in the label |
| A claim from a video/podcast | `[[<source-basename>#^t1226\|20:26]]` | source badge: kind icon · short name · time; click plays it in place |
| A claim from a book/article | `[[<source-basename>\|p. 42]]` | source badge with your locator |
| A Hebrew/Greek term | `[[Herem\|ḥērem]]` (a page with `lemma:`) | the word in its script + transliteration; gloss on hover |
| Another page | `[[Page]]` or `[[Page\|display]]` | link; its summary on hover |

- Cite the source **after the sentence it supports**, before the full stop.
- Only anchors that exist: a transcript's `^t<sec>` marks are the start
  of each caption segment. Cite the segment that contains the claim.
- The same source twice in a row shortens itself (icon · first word ·
  time) — don't abbreviate by hand.
- A source page should carry `short_title:` (2–4 words) and `author:`;
  the badge and the one-voice check read them.
- Link a Hebrew/Greek term **once per page**, at its first use; after
  that, italic transliteration.

### Verse cards — when the words themselves matter

A reference alone on its own line shows the verse. Add your note after an
em dash:

```markdown
[[bible::Eph.6.12|Ephesians 6:12]] — Paul's fullest list of the powers, and the claim that they, not people, are the enemy.
```

- Use a card when the reader needs to *read the text* to follow the
  argument. Use a badge when they only need to know where it is.
- The note says what the verse **adds** — never a paraphrase of it.
- A run of cards separated by blank lines replaces a “passage | what it
  says” table: the reader gets the text instead of a summary of it.
- Not in a list item (`- [[…]] — …` stays an inline badge).

### Callouts — a box with a job

```markdown
> [!question]- How firm is this?
> Body lines, each starting with `>`.
```

`-` after the kind starts folded, `+` foldable but open, neither = always
open. Kinds (aliases in brackets) and their one job here:

| Kind | Use for |
|---|---|
| `abstract` (`summary`, `tldr`) | the whole page in 3 bullets, on long pages only |
| `question` (`faq`, `help`) | **folded**: how firm is this / contested / other readings / what this does not settle |
| `note` | an aside the argument does not need |
| `info` | background a newcomer needs (who, when, what text) |
| `tip` (`hint`, `important`) | how to read or use something |
| `warning` (`caution`, `attention`) | a common misreading, a known error in a source |
| `quote` (`cite`) | a quotation longer than a sentence, with its badge |
| `example` | a worked example |
| `success` / `failure` / `danger` / `bug` / `todo` | not for study pages (project and code pages) |

- At most one open callout per section; folded ones as needed.
- Tables do not render inside callouts — put a table after one.

### Readings — a contested question

````markdown
```readings
Who are the gods the psalm puts on trial?
## Human judges
Israel's rulers, called gods because they judged in God's name.
+ Fits the charge of unjust judgment.
- A death “like men” only lands on someone who is not a man.
held: Much traditional Christian commentary
## Divine beings
The council of [[Elohim|elohim]] given the nations.
+ “The assembly of El” is Ugarit's phrase for the council.
held: Most scholarship since Ugarit
verdict: This wiki follows the divine-beings reading.
```
````

- Use it whenever a page would otherwise say “some think X, others Y”.
- 2–3 positions (more stacks badly). Each gets its best case (`+`) and
  its real weakness (`-`); a position with no `-` is a strawman of the
  other.
- `verdict:` says where the page lands, or that it doesn't.
- Links, badges and emphasis work inside.

### Timeline — a sequence with dates

````markdown
```timeline
1928 | A farmer's plough opens a tomb near Minet el-Beida.
1929 | The dig at Ras Shamra; the first tablets.
```
````

For discoveries, scholarship, a reign, a life. `date | event`, one per
line; dates as the reader would say them (`c. 1200 BC`, `AD 70`). A
bulleted list with years in bold is a timeline — use this.

### Map — places relative to each other

````markdown
```map
title: From Jerusalem toward Tarshish
Jerusalem | 31.78, 35.23 | where the mission starts
Rome | 41.90, 12.50
route: Jerusalem > Antioch > Rome
```
````

`Name | lat, lon | note`; `route:` lines join places in order (several
allowed). Drawn to scale without coastlines, so it shows *where things
are relative to each other* — use it for journeys, campaigns, lists of
cities. Coordinates to two decimals; look them up, never guess.

### Tabs — the same thing several ways

````markdown
```tabs
=== WEB
In the beginning…
=== KJV
In the beginning…
```
````

For parallel versions of one thing — translations, the same example in
two forms. **Links and badges do not resolve inside tabs**: plain text
and emphasis only.

### Tables — genuinely two-dimensional data

Rows that share columns: verse → what happens, who → where → when. Not
for two columns of “term: definition” (a list), not for pros/cons
(readings), not for a passage and its text (verse cards).
Escape the pipe in a link's display text inside a table: `[[bible::Ps.82.1\|Psalm 82:1]]`.

### Section embeds — one source of truth

`![[Three Rebellions#At a glance]]` on its own line shows that section,
rendered in full, with a head that opens it. Use it when two pages need
the same summary table or list. Give the embedded section its own
heading on the page that owns it.

### Inline marks

- `**bold**` — the one phrase a skimming reader must not miss; once or
  twice a page.
- `*italic*` — titles of works, transliterations, a word used as a word.
- `==highlight==` — not on study pages (it reads as a student's marker).
- `^[inline footnote]` — a qualification that would break the sentence.
- `%%comment%%` — a note to the next editor; invisible when reading.
- `#tag` in text — no; tags live in frontmatter.
- Not available in Task: mermaid diagrams, `$math$`. Don't write them.

## House rules

- One paragraph per line; blank line between paragraphs.
- Curly quotes (“ ” ‘ ’), en dash in ranges (`2–4`), em dash for breaks
  (` — `), `c.` for circa, `BC`/`AD` after/before the number.
- Scripture labels in full book names (`1 Corinthians 15:24–25`), never
  the OSIS code; the singular `Psalm` for one psalm.
- Headings in sentence case; a question heading ends with `?`.
- Numbers: words for one to nine in prose, figures for anything
  measured or cited.
- Name scholars in full the first time (`Hans-Joachim Kraus`), surname
  after.
- Say who claims what: “the video argues”, “Heiser reads” — the wiki’s
  own voice only for what it can stand behind.
- AI-written pages keep `ai_generated: true` and `generated_by:` in the
  frontmatter.

## Anti-patterns

| Instead of | Write |
|---|---|
| a summary paragraph that repeats `summary:` | start with what comes *after* the summary |
| “Reading A” / “Reading B” paragraphs with bold labels | a ```` ```readings ```` block |
| a table of passages with paraphrases of each | verse cards with notes |
| `**1928** — …` bullets | a ```` ```timeline ```` block |
| a numbered list of cities on a route | a ```` ```map ```` with `route:` |
| “## Contested” as an open section at the bottom | `> [!question]- Contested` |
| the same table on two pages | the table on one, `![[Page#Heading]]` on the other |
| `bible::Josh.11.21` as visible text | `[[bible::Josh.11.21\|Joshua 11:21]]` |
| a bare timestamp `(20:26)` | `[[source#^t1226\|20:26]]` |
| decorative callouts, emoji headings | nothing — let the structure carry it |

## Before you save

1. Title once; `summary:` present and ≤ 140 characters.
2. Every `[[link]]` names a page that exists (or is meant to be created);
   every `#^t…` anchor exists in its transcript.
3. Every `bible::` target is `Book.C.V` or `Book.C.V-Book.C.V`, with a
   full-name label.
4. Contested or uncertain material is folded, not open.
5. No element used against its job (the table above).
6. Read the page top to bottom with only the lede and the first
   paragraph of each section: does it still make its point?

Then upload (`task wiki page write <wiki> <path> --from <file>`) and
open it in the app — the editor is the renderer, and a block that does
not parse shows as source, which is the fastest check there is.
`task wiki gaps --wiki <slug>` lists pages that rest on one voice.

## Restyling an existing page

1. Read it; list its claims in order. That list is the page.
2. Write the `summary:` from the first claim.
3. Map each section to the element that fits its shape (the “which
   element” sections above). Most restyles are three moves: a table → verse cards or
   readings, a closing “contested” section → a folded callout, bold-year
   bullets → a timeline.
4. Keep every citation; restyling never drops evidence.
5. Diff before and after: the words should mostly survive, rearranged.
