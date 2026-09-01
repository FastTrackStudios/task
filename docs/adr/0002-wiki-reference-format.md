# 2. Wiki references are readable, qualified, and stamped

Date: 2026-09-01

## Status

Accepted.

## Context

A vault or wiki page references content in another wiki or Resource with
wikilink syntax. What goes in the raw markdown is a decision that reaches
every file in the system and is expensive to revisit, because changing it
means rewriting everyone's prose — including prose held in local copies on
machines we do not control.

Four forces pull against each other:

- **Outside tools must be able to read it.** `wiki.local.mount` puts every
  wiki and subscribed source in the file sync clients as plain markdown.
  `wiki-proto`'s module docs promise pages round-trip through Obsidian and
  that a wiki directory is portable to `llm_wiki`. A file full of opaque
  ids satisfies none of that.
- **Agents and scripts prefer stable keys.** An id that survives a rename
  is easier to process than a title that does not.
- **Names collide.** Two orgs may both publish a wiki called `theory`. A
  bare `[[Ionian]]` or a short `theory::Ionian` means different things in
  different vaults, and the failure is silent.
- **Sections have no stable names.** Referencing part of a page by heading
  breaks when the heading is retitled, which is the common case.

The parser already resolves `[[Page]]`, `((uuid))`, `![[Page#Heading]]` and
`![[Page#^short-id]]` (`vault-live/src/lookup.rs`), so this is not a
question of what can be read. It is a question of what the editor writes.

Logseq's approach — uuids in the file, prettified in the editor — was
considered and rejected. It buys rename-stability that history already
provides (`wiki.link.repair`) at the cost of the one property the mount
promise depends on.

## Decision

A reference is **readable, qualified by federation domain, and stamped with
the moment it was made.**

```
[[acme.test/music-theory::Ionian@2026-09-01]]
[[acme.test/music-theory::Harmonic Series@2026-09-01#^partials]]
[[acme.test/music-theory::Ionian@2026-09-01|Ionian]]
```

`<domain>/<wiki-slug>::<Page>[@<stamp>][#^<anchor>][|<display>]`. The
stamp is a date on the target rather than in the alias slot, so the
alias stays available for display text and an outside markdown editor
still renders something sensible.

1. **Readable, not opaque.** The target's slug appears literally. Raw
   markdown is noisier than `[[Ionian]]` and that is accepted: legibility
   to Obsidian, to `llm_wiki`, and to an agent reading raw files is worth
   more than width.
2. **Qualified by the publishing org's federation domain.** DNS already
   settles uniqueness, so two orgs cannot collide, and a link copied from
   one vault into another still names the same page.
3. **Block references use `^short-id`**, Obsidian's own block-anchor
   format — rename-proof and section-precise for six characters, and the
   one form Obsidian itself will also resolve in a mounted folder.
4. **Every reference carries when it was made.** The link resolves to the
   *current* page always; the stamp exists so staleness is derivable —
   "this has changed four times since you linked it" — and so the version
   the author actually saw can be recovered from history when something
   looks wrong. The stamp never pins.
5. **Authoring never requires typing this.** `[[` opens a picker over
   everything the writer subscribes to; the qualified form is what gets
   inserted, and the editor renders it as the short title.

### The domain is a name, not an address

A wiki keeps its original qualified reference for life, including after its
publishing org is gone and another org has adopted it. Resolution goes
through the org registry, which retains a redirect for a departed org;
subscribers follow the redirect without a byte of their prose changing.

This means the domain in a reference is a *historical fact about who first
published it*, not a live location — a wiki can be permanently named after
an org that no longer exists. That is the accepted cost. A command exists
to rewrite ids across a vault for anyone who wants the tidier name, but it
is invoked deliberately and never runs on its own.

## Consequences

**Good.** Files stay readable everywhere they are opened. Collisions are
impossible rather than unlikely. Staleness is computable without extra
state, and the "what did this say when I cited it" question is answerable.
Adoption preserves every existing reference.

**Bad.** Prose is visibly noisier, and long references wrap badly in a
narrow editor — mitigated by rendering the short title, but the raw file is
what a mounted-folder reader sees. A wiki may end up named after a dead
org, which will surprise people. Redirects become security-relevant: they
are how a reference is repointed, so their authorization matters (a signed
handover from the departing org, or per-subscriber acceptance when no
signature exists).

**Reversible only forward.** Changing this later means rewriting prose in
copies we do not control. The id-rewrite command is the escape hatch, and
it is opt-in by design.
