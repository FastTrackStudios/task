---
target: Tasks page
total_score: 20
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 2
target_identity: "file:/run/media/Development/task/crates/ui/src/pages/tasks.rs"
target_fingerprint: "sha256:878f40e7b727762dfdb682fdeb3760e0f1b9a77c78369a3649e37400955ea4c1"
target_path: /run/media/Development/task/crates/ui/src/pages/tasks.rs
timestamp: 2026-10-06T22-44-14Z
slug: crates-ui-src-pages-tasks-rs
---
Method: dual-agent (A: design-review sub-agent · B: detector-and-browser-evidence sub-agent)

# Critique — Tasks page (crates/ui/src/pages/tasks.rs → features/task/task-ui)

## Design Health Score
| # | Heuristic | Score | Key Issue |
|---|---|---|---|
| 1 | Visibility of System Status | 2 | In-progress rows pulse "Tracking" with no clock running; "0 done" beside "3 hidden" (the done ones) |
| 2 | Match System / Real World | 2 | Generic vocabulary ("Buy milk #errands", "Relevant", "anywhere"); no production language |
| 3 | User Control and Freedom | 2 | Sheet trash deletes instantly, no undo; Cancel can't undo instant-saved fields |
| 4 | Consistency and Standards | 2 | Two delete patterns; kanban drops project chip; note-editor Edit/Read/Raw on a task list |
| 5 | Error Prevention | 2 | Check-off starts a timer; delete next to close |
| 6 | Recognition Rather Than Recall | 2 | Quick-add syntax unpreviewed; Relevant explained only in tooltip; unlabeled red sidebar badge |
| 7 | Flexibility and Efficiency | 1 | No keyboard model, no bulk reschedule, kanban not draggable, view not remembered |
| 8 | Aesthetic and Minimalist Design | 2 | 40px title vs 10px due pills; unfiled tasks shown twice; two stacked add buttons on phone |
| 9 | Error Recovery | 3 | ErrorState+Retry, optimistic rollback; no undo toast |
| 10 | Help and Documentation | 2 | Checkbox cycle and relevance rules undocumented in UI |
| Total | | 20/40 | Acceptable |

## Design Specificity Verdict
Category-interchangeable Things/Todoist pattern; nothing says production team (no owners, org/client, deliverables, client view). Checkbox-as-timer and the Now bar are half-landed product ideas. Detector: .rs dir scan gives a false clean; explicit .rs scan 1 finding (side-tab toast, FP); live URL scans 42 desktop / 43 phone — undersized-ui-text ×31 (10px pills), low-contrast ×2 (primary #fbfbfb on #7f6df2 3.8:1; avatar 2.5:1), pulsing-dot, margin transition, AI-palette on avatar/covers. Phone: 27/35 targets <44px (checkbox 24, ⋯ 28). Muted grey passes AA (5.9–6.9:1, dark themes). Overlay injected in Chrome tab "[Human] Task — critique"; its console summary crashes (circular JSON via Dioxus patch_console).

## Priority Issues
- [P1] Overdue work isn't the loudest thing on screen — in-progress rings lead, overdue = 10px pill at far right, amber triage strip outranks. Fix: Overdue band in Today with age + bulk "Move all"; demote triage. (/impeccable layout)
- [P1] The checkbox isn't a checkbox — first tap starts tracking (tasks_app.rs:182-195), "Tracking" pulses without a session (row.rs:280-302). Fix: tap=complete+undo, separate ▶ to track, pulse only with a live session. (/impeccable shape, /impeccable harden)
- [P2] Counts and sidebar disagree under filters — header 0 done vs 3 hidden; sidebar ignores Relevant/location; Unfiled click no-op on /tasks; no active section. Fix: one filtered source for all counts; Unfiled opens strip; highlight in-view section. (/impeccable harden)
- [P2] Primary action fails keyboard/SR/touch — div onclick rows, unnamed checkbox, invisible focus, 24/28px phone targets, 3.8:1 primary buttons, no reduced motion. (/impeccable audit, /impeccable adapt)
- [P2] Detail sheet mixes save models + unguarded delete + misleading "No project". Fix: autosave + Saved tick, delete in overflow with undo, "Project in ‹org›". (/impeccable harden, /impeccable clarify)

## Persona Red Flags
- Alex: no shortcuts, no bulk reschedule, silent quick-add parse, view resets, read-only kanban.
- Sam: unnamed checkbox, non-focusable titles, invisible focus, hue-only 10px urgency, endless pulses, 3.8:1 Save.
- Casey: tap starts timer, add sheet doesn't focus input, truncated placeholder, 24/28px targets, FAB covers ⋯, ~22px Now-bar buttons.
- Studio engineer: only "Tasks" + amber strip readable at arm's length; Today count includes undated in-progress; false "Tracking" pulses; no late/next line; Now bar gone when idle.

## Minor Observations
"3 hidden" wraps at 895px; location select unlabeled; triage copy omits the @context rule; contradictory comments row.rs:36 vs tasks.rs:194; empty Done kanban column; hard-coded dark chip colours (row.rs:98,107); "Cancel (hide)" wording; seeded titles carry project names; detector FPs (side-tab toast, segmented padding, hidden phone duplicates, scroll-column clipping).

## Questions to Consider
- What single line should one glance between takes give?
- Why does a team list show no faces?
- Should ticking a box ever start a clock?
- Is the amber strip for the person or the data model?
- What says "this studio has its act together" to a client that Todoist couldn't?
