LoroText upgrade per plans/loro-text-editor-upgrade.md, Phases 1+2 only (skip Phase 3 chat).

Done when ALL hold AND evidence is in transcript:

P1 — crdt lib (../architect/libs/crdt/src/codec.rs):
- `text_child`, `read_text`, `apply_text_ops`, `apply_text_diff`, `TextOp` exist (show grep).
- `cargo test -p crdt` exits 0 with new tests: get-or-create idempotence, insert+delete round-trip, two-LoroDoc concurrent merge at same pos, common-prefix diff minimality.

P2 — knowledge:
- `BlockEntity` (features/knowledge/knowledge-crdt/src/lib.rs) uses LoroText for content; no `write_str(m, "content", ...)` remains (grep).
- `BlockRepoLoro::apply_text_ops` exists.
- `editor::input` returns `Vec<TextOp>` in InputCommand.
- `BlockEditor` uses `onbeforeinput` + prevent_default; `oncomposition*` + `onpaste` wired; blur-save removed.
- Migration-on-read helper handles legacy string content idempotently.
- `cargo test -p knowledge-proto -p knowledge-crdt -p knowledge-ui` exits 0.
- `cargo check -p task-ui` and `cargo check -p task-app-web --target wasm32-unknown-unknown` exit 0.

Commits: one per phase on feat/architect-migration, message references plans/loro-text-editor-upgrade.md. Show `git log --oneline -5`.

Constraints:
- Block.content public Rust shape stays `String`.
- Unknown beforeinput inputTypes fall back, never panic.
- No compat-shim/removed comments; delete cleanly.
- Fix root causes; no --no-verify.
- Stop after 40 turns; report blocker if so.

Each turn: state which item just satisfied, which next.
