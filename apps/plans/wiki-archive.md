# Wiki archive — URL front door for the raw→ingest pipeline

**Status:** shipped — `features/task/wiki/wiki-archive` exists and `task wiki archive` is live.

`task wiki archive <url|file>` routes a URL through a
content-type router to an extractor, stamps the extracted
markdown with provenance frontmatter (`source_url`,
`canonical_url`, `content_type`, `archived_at`, `extractor`,
`media`, `duration`), and feeds it to the UNCHANGED
raw→ingest pipeline (`import_raw_source` sha-dedup + ingest
queue). Crate: `features/wiki/wiki-archive`.

Tracked issues: phase 1 `32e7b28f` (router, articles,
YouTube `^t<sec>` transcripts, Readwise/Karakeep/Pocket/
Netscape importers, SourceViewer), phase 2 `0a09a0d5` (PDFs
with page anchors, podcasts/whisper, audio player), phase 3
`6397f305` (social, extractor health).

## Conventions

- Archived source filenames: `<title-slug>-<canon8>.md` where
  `canon8` = first 8 hex of sha256(canonical URL). Dedup is a
  filename scan — no frontmatter parsing, no extra RPC.
- Timestamped media: transcript coalesced into ~45 s blocks,
  each anchored `^t<seconds>` (legal under the existing
  obsidian block-anchor grammar — `[[Sources/x#^t870]]` deep
  links work with zero parser changes). Curator notes go under
  `## Notes` as `- [mm:ss] … ^t<sec>-noteN`.
- PDFs: the first paragraph of every page carries a 1-based
  `^p<page>` anchor (`[p. 12] … ^p12`), same grammar. Pages
  with no extractable text render an honest placeholder —
  OCR is deferred, not faked. Engines: pdfium when bindable
  (`pdf` feature + `TASK_PDFIUM` dir or system libpdfium —
  dynamic binding, nothing bundled, mirroring wiki-extract),
  else `pdftotext -layout`.
- Podcasts: `media:` frontmatter = the playable enclosure
  URL; the SourceViewer renders a native `<audio>` player and
  `[mm:ss]` chips seek via `currentTime`. Transcript ladder:
  feed `<podcast:transcript>` tag (fast path, no audio fetch)
  → `--transcribe groq` backfill (GROQ_API_KEY) → local
  whisper (`--features whisper` build; whisper-rs pinned to
  its codeberg home — the GitHub repo is archived; `small`
  model dev default, `large-v3-turbo` for production) →
  honest "no transcript" note. Spotify is metadata-only
  (no public audio/transcript); PODCASTINDEX_API_KEY/SECRET
  enables title-search resolution back to public RSS.

## Phase 3 — social, honestly fragile

The Reddit/X routes are **accept-fragility tier**: both scrape
surfaces that drift every few months, and the design response
is honesty infrastructure, not pretended stability. Standing
maintenance is EXPECTED here — when a route breaks, the fix is
a small parser/endpoint patch, and `task wiki archive health`
is the surface that says so (per-org ledger of last
success/failure per route, recorded on every attempt).

- Reddit: anonymous loid-cookie dance (GET old.reddit.com once
  for cookies → `<permalink>.json` two-Listing parse). The
  cookieless `.json` endpoint is dead (403, live-verified
  2026-06). Three live-found tripwires, all handled: a
  self-identifying UA gets no loid (stock-browser UA only); a
  MISSING `accept` header gets 403 (always send `accept: */*`);
  and Reddit's edge fingerprints rustls TLS — when the
  in-process client is blocked, the same dance runs through a
  `curl` subprocess (OpenSSL fingerprint), which sails through.
  ≤ ~10 req/min; content-type-checked before parse (block
  pages 200 as HTML).
- X ladder: syndication tweet-result (token = V8-faithful port
  of react-tweet's `((id/1e15)·π).toString(36)`; note-tweet
  truncation detected and escalated) → FxEmbed → vxtwitter →
  official oEmbed (text-only) → unarchived stub. Every field
  optional; the rung that answered is recorded in `extractor:`.
  Nitter is deliberately not a dependency.
- Unarchived stubs: a blocked accept-fragility extraction
  stores `unarchived-<slug>-<canon8>.md` with
  `archive_status: unarchived` + the error in frontmatter.
  `task wiki archive retry` (cron-friendly: throttled, exits
  0) sweeps stubs, replaces them with the real source on
  success. A direct re-archive of a stubbed URL also replaces
  the stub.

## Phase-1 follow-ups

- Binary originals → `Wiki/media/archive/<sha-prefix>/`
  (content-hash-addressed). Needs a media-write RPC on the
  wiki surface; today only `raw/sources/` lands via RPC, so
  videos record `media:` as the canonical watch URL instead.
- `--force` re-archive imports a fresh copy (suffixed file)
  rather than overwriting — RawLayer has no overwrite verb.
- yt-dlp runs as a CLI-side subprocess with retries; the
  server-side background-job version (retryable, never
  save-blocking) lands when archiving moves behind an RPC verb.
- bgutil POT-provider sidecar for YouTube bot-checks: not
  wired; "update yt-dlp" is the standing fix.
- SourceViewer v1 (`/wiki/source/:name`) is read-only with
  seek-on-anchor-click (IFrame postMessage). The "note at
  current time" button (getCurrentTime → insert
  `- [mm:ss] … ^t<sec>-noteN` under `## Notes` on the
  generated source page) is the next slice — it needs a
  write path to wiki pages from the web app.
- Imported bare bookmarks land as `content_type: bookmark`
  without fetching; canonical dedup then blocks a later full
  `task wiki archive <url>` of the same page unless --force.
  A `task wiki archive upgrade <source>` verb (re-extract in
  place) would resolve this cleanly.
