---
name: sermon-sync
description: Keep a YouTube channel's message videos synced into a wiki (the Bible Study wiki's `Resources/Sermons/`) as sermon resources — the video link plus its captions, with a `Sermons.base` over them — so every sermon can be opened at a timestamp, annotated, referenced from wiki pages, and found from the scripture it quotes. No model anywhere; a cron job runs it.
runs_as: any writer of the org (the account whose CLI session runs the cron)
trigger: "sync the Crossroads sermons", "add the church's YouTube channel", "why doesn't this sermon show up on 1 Peter 5", "set up the nightly sermon sync"
---

# Sermon sync — a channel's messages as resources

`task resources sermons sync` walks a channel's videos tab, fetches
each new video's captions with yt-dlp (uploader captions preferred,
YouTube's auto captions as the fallback), and hands the server a
`SermonResource`. The server writes the resource, extracts the
scripture references the preacher *spoke* (`first Peter chapter five
verse seven`) or that the captions *wrote* (`1 Peter 5:7`), and mints
one typed link per reference — `sermon:<slug>#t:<secs> → verse:<osis>`.
That link is what makes the sermon appear on the verse in the
scripture reader.

Nothing in the loop calls a model. It is a grammar over the cues
(`resources::scripture_refs`) and file writes.

## 1. Run it

```bash
# once, so the CLI is signed in against the deployment (skills/live-dev.md §3)
export TASK_SESSION_FILE=$SCRATCH/session.json
export TASK_YTDLP="$(nix shell nixpkgs#yt-dlp -c which yt-dlp)"   # or a yt-dlp on PATH

# the whole channel into the Bible Study wiki (skips ids already synced;
# exits 0 once the listing worked)
task --server https://task.fasttrackstudio.app resources sermons sync \
  https://www.youtube.com/@CrossroadsCA --org codywright --wiki bible --folder crossroads

# one video
task --server https://task.fasttrackstudio.app resources sermons sync-one \
  https://youtu.be/YMypVgZXFIU --org codywright --wiki bible --folder crossroads

# a folder that was synced into the org-wide tier, moved into the wiki
task --server https://task.fasttrackstudio.app resources sermons move \
  --org codywright --folder crossroads --wiki bible

# what the server holds
task --server https://task.fasttrackstudio.app resources sermons list --org codywright
```

Flags that matter:

| flag | meaning |
|---|---|
| `--wiki <slug>` / `TASK_WIKI` | the named wiki the sermons belong to (`bible`): pages under `<wiki>/Resources/Sermons/<folder>/`, with `Sermons.base` beside the folders. Without it the org-wide `resources/sermons/` tier — only for resources no single wiki owns. |
| `--folder <name>` | subfolder under the sermons root; also the default second tag. One per channel (`crossroads`). |
| `--tag <t>` (repeatable) | `tags:` for new sermons; default `sermon` + the folder name |
| `--since YYYY-MM-DD` | stop at the first video older than this (the tab is newest-first) |
| `--limit N` | at most N new videos this run — use it the first night on a big channel |
| `--dry-run` | list what would be synced; fetch nothing, write nothing |
| `--yt-dlp <bin>` / `TASK_YTDLP` | the binary; nothing is bundled — "update yt-dlp" is the standing fix when YouTube changes |
| `--pause <secs>` | wait between videos (default 3; yt-dlp rate-limits) |
| `--channel-name <name>` | `writers:` override when the channel's display name is not what you want |

The channel accepts `https://www.youtube.com/@Handle`, the same with
`/videos`, a bare `@Handle`, or a channel id (`UCA82jGZtU5BAIwkw5bsuBhg`).

**Always pass `--server`** against a deployment (see `live-dev.md` §3 —
without it the CLI can fall back to an embedded backend on this
machine and "succeed" invisibly).

## 2. Nightly

A `just` recipe wraps the command for the deployment:

```bash
just sermon-sync                 # whole channel, from .env's TASK_LIVE_SERVER
just sermon-sync --limit 5       # extra args pass through
```

A systemd user timer, for a box that has the CLI session file:

```ini
# ~/.config/systemd/user/sermon-sync.service
[Unit]
Description=Sync Crossroads sermons into Task

[Service]
Type=oneshot
Environment=TASK_SESSION_FILE=%h/.config/task/session.json
Environment=TASK_YTDLP=%h/.nix-profile/bin/yt-dlp
ExecStart=%h/.cargo/bin/task --server https://task.fasttrackstudio.app \
  resources sermons sync https://www.youtube.com/@CrossroadsCA \
  --org codywright --folder crossroads

# ~/.config/systemd/user/sermon-sync.timer
[Unit]
Description=Nightly sermon sync

[Timer]
OnCalendar=*-*-* 03:30
Persistent=true

[Install]
WantedBy=timers.target
```

`systemctl --user enable --now sermon-sync.timer`. The command exits 0
whenever the channel listing worked, so the unit only fails when
YouTube (or the session) is actually broken; the summary line —
`synced N, skipped M already present, K without captions, E errors` —
is the thing to read in `journalctl --user -u sermon-sync`.

## 3. What it writes

Under `<org>/wikis/<wiki>/Resources/Sermons/<folder>/` (or, without
`--wiki`, `<org>/resources/sermons/<folder>/`). The first sermon into a
wiki also lays down `Resources/Sermons/Sermons.base` — a table of every
sermon newest-first and a board by channel — which is never rewritten,
so reshape its views freely. Per sermon:

| file | owner |
|---|---|
| `<slug>.md` | frontmatter split: the sync rewrites `media`, `published`, `duration_secs`, `tags`, `scripture`, `source`, `caption_kind`, `language` on every run; `title`, `writers`, any key you add, and the **whole body** are yours and survive re-syncs |
| `<slug>.transcript.json` | the sync's — `{"slug","source":"youtube-auto"|"youtube-manual","segments":[{"start","dur","text"}]}` |
| `<slug>.annotations.json` | created empty once; never rewritten |

Frontmatter of a fresh sermon:

```yaml
type: resource
resource_kind: sermon
slug: god-restores-broken-people
title: God Restores Broken People
writers: [Crossroads Church]
readonly: true
tags: [sermon, crossroads]
published: 2026-06-14
duration_secs: 2856
media:
  - kind: video
    provider: youtube
    url: https://youtu.be/YMypVgZXFIU
    id: YMypVgZXFIU
source: youtube-captions
caption_kind: auto
language: en
scripture: [1Pet.5.7, John.21.15-John.21.17, 1Pet.5]
```

The body is the `# Title` line, a `*Channel · [video](url) · MM:SS*`
line, and a `## Notes` section with a one-line placeholder. The sync
never writes an outline — that is hand work, and the second run keeps
whatever you wrote.

The slug is the kebab-case title. It belongs to the **video id**: a
renamed video keeps its slug (found through `media[].id`), and a second
video with the same title gets the id appended
(`hope-bbb123xyz00`).

In the links store (`<org>/links.jsonl`), one link per reference per
moment, `source_ref: sermon-sync`, confidence `Likely` for a written
verse, `Possible` for a spoken verse or a written chapter, `Speculative`
for a spoken chapter-only mention. A re-sync deletes the sermon's
`sermon-sync` links and mints them again; your own annotations on the
sermon are other links and are left alone.

## 4. Opening, annotating, referencing

- **Open**: the FastTrackStudio app's Watch screen, `Watch` in the
  rail → the Sermons list, or directly by node:
  `/app/fasttrackstudio?q=<packed "node=sermon:<slug>&t=109">` (the
  `resources_ui::sermon_href(slug, secs)` helper builds it). The video
  is embedded and playable; the transcript lines beneath seek the
  player on click and pre-fill the note box; `t` lands the player at
  that second.
- **Annotate**: "Add at current time" on the Watch screen writes a
  typed link from `sermon:<slug>#t:<secs>` (a note, optionally linked to
  a verse or topic). Those are yours; the sync never touches them.
- **Reference from a wiki page or note**: the anchor form is
  `sermon:<slug>#t:<secs>` — write it as a NodeRef token in a typed
  link, or as prose (`sermon:god-restores-broken-people#t:109`) that a
  reader can paste into the Watch screen. A study note keeps
  `sermon: <slug>` and `youtube:` in its frontmatter (`type: study`,
  `kind: sermon`) and links the scripture with `[[1 Peter 5:7]]`.
- **From scripture**: open the passage in the reader (`/scripture`,
  e.g. 1 Peter 5). Verses the sermon named show the backlink
  `↳ God Restores Broken People · 1:49`; clicking it opens the sermon
  at 1:49. Chapter-only mentions (`Romans 8`) sit under "Mentions this
  chapter" at the top of the chapter.

## 5. The Bible Study wiki

Synced with `--wiki bible`, the sermons *are* pages of the Bible Study
wiki: `Resources/Sermons/crossroads/<slug>.md` shows in the wiki's
explorer (folder view) and under its `sermon` / `crossroads` tags, opens
in the wiki editor with the same right sidebar (Properties, Links,
Graph, Share), and travels with the wiki's subscriptions. The
`Resources/Sermons/Sermons.base` table is the collection view. The
resource tier is scoped to the wiki on purpose: a sermon collection is
not a continuously updated org-wide feed, it is study material that
belongs to this one subject — the same way the cooking wiki keeps its
recipes under `Cookbook/`.

Per-passage pages can carry `sermon:<slug>#t:<secs>` anchors next to
the verse they discuss, and `task resources sermons list --json` gives a
curated index page everything it needs (slug, title, published,
`scripture`, `wiki`).

## 6. When it looks wrong

| Symptom | Cause | Fix |
|---|---|---|
| `yt-dlp not found` | not on PATH | `--yt-dlp "$(nix shell nixpkgs#yt-dlp -c which yt-dlp)"` or `TASK_YTDLP` |
| `probe failed … after 2 attempts` | YouTube rotated a bot check | update yt-dlp; the video is retried next run (it is not marked synced) |
| `K without captions` | the video has neither uploader nor auto captions | nothing to do; the summary names the ids |
| Sermon missing on a verse it clearly quotes | the caption phrasing is outside the grammar | check `scripture:` in the frontmatter; the extractor's unit tests in `features/resources/resources/src/scripture_refs.rs` are where a new spoken form gets added |
| A sermon appears twice | the video was re-uploaded under a new id | expected: one resource per video id; delete the old `.md` and its sidecars if the old id is gone |
| Synced but the web app shows nothing | the CLI talked to a different server (embedded fallback) | `--server`; `task auth whoami` |
