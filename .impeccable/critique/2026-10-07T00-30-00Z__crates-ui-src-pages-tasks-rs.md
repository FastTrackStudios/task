---
target: Tasks page
total_score: 29
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 0
target_identity: "file:/run/media/Development/task/crates/ui/src/pages/tasks.rs"
target_path: /run/media/Development/task/crates/ui/src/pages/tasks.rs
timestamp: 2026-10-07T00-30-00Z
slug: crates-ui-src-pages-tasks-rs
previous: 2026-10-06T22-44-14Z (20/40)
---
Method: single pass after the fix batch (commit 7f63ffa + shell polish) — desktop in Chrome, phone 390×844 headless, detector on source and live URL.

# Re-critique — Tasks page

## Design Health Score
| # | Heuristic | Was | Now | Note |
|---|---|---|---|---|
| 1 | Visibility of System Status | 2 | 3 | Overdue band leads Today with age; pulse only with a live clock; header names what is hidden |
| 2 | Match System / Real World | 2 | 3 | Owners and org on rows; "late", "Won't do", "No project" |
| 3 | User Control and Freedom | 2 | 3 | Undo on complete/won't-do/delete; sheet autosaves |
| 4 | Consistency and Standards | 2 | 3 | One delete pattern; one count source; kanban shows the same chips |
| 5 | Error Prevention | 2 | 3 | Checkbox completes, never starts a clock; delete is two-step in a menu |
| 6 | Recognition Rather Than Recall | 2 | 3 | Quick-add previews its parse; "I'm at" labelled; "N late" badge |
| 7 | Flexibility and Efficiency | 1 | 2 | Bulk reschedule, view remembered; still no keyboard model, kanban not draggable |
| 8 | Aesthetic and Minimalist Design | 2 | 3 | No 10px text on the board; unfiled strip demoted below the list |
| 9 | Error Recovery | 3 | 4 | Undo toasts on every regrettable action |
| 10 | Help and Documentation | 2 | 2 | Relevance rules still explained only by tooltip |
| Total | | 20 | 29 | Good |

## Detector
Source files: clean. Live URL: 42 → 15. Left: the account avatar (initials 10px inside a titled disc; Alice's sky→blue gradient reads as the "cyan" tell), Inter as the theme font (user-selectable in Appearance), two padding notes on wrappers. Primary-button and avatar contrast now pass (runtime guard; 700-shade avatar palette with a test).

## Next, if wanted
- [P2] Keyboard model: j/k, x complete, e edit, t today, and a `?` sheet. (/impeccable harden)
- [P2] Kanban drag between columns. (/impeccable shape)
- [P3] Say what Relevant hides in the list itself, not a tooltip. (/impeccable clarify)
