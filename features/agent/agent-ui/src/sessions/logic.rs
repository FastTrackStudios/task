//! Pure, tested derivation logic for the agent command center —
//! the t3code `*.logic.ts` pattern: components stay thin `rsx!`,
//! everything decidable lives here as plain functions.

use agent_proto::service::discovery::ModelInfo;
use agent_proto::session::SessionStatus;

/// Weighted completion ranking (CodexMonitor's `scoreMatch`):
/// exact > name-prefix > substring > subsequence. `None` = no match.
pub fn score_match(query: &str, name: &str) -> Option<i32> {
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if n == q {
        return Some(110);
    }
    if n.starts_with(&q) {
        return Some(95);
    }
    if n.contains(&q) {
        return Some(70);
    }
    // Subsequence: every query char appears in order.
    let mut chars = n.chars();
    if q.chars().all(|qc| chars.by_ref().any(|nc| nc == qc)) {
        return Some(50);
    }
    None
}

/// Rank rows by `score_match` over their name, stable-sorted by
/// score desc then name asc, capped at `max`.
pub fn rank_by<T>(items: &[T], query: &str, name_of: impl Fn(&T) -> String, max: usize) -> Vec<T>
where
    T: Clone,
{
    let mut scored: Vec<(i32, String, T)> = items
        .iter()
        .filter_map(|it| {
            let name = name_of(it);
            score_match(query, &name).map(|s| (s, name, it.clone()))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(max).map(|(_, _, it)| it).collect()
}

/// One rail status pill (t3code's priority-ranked model, adapted
/// to our single-valued `SessionStatus`).
pub struct StatusPill {
    pub label: &'static str,
    /// Tailwind classes for the dot.
    pub dot: &'static str,
    pub pulse: bool,
}

pub fn status_pill(status: SessionStatus) -> Option<StatusPill> {
    match status {
        SessionStatus::AwaitingUser => Some(StatusPill {
            label: "Awaiting input",
            dot: "bg-amber-500",
            pulse: true,
        }),
        SessionStatus::Running => Some(StatusPill {
            label: "Working",
            dot: "bg-primary",
            pulse: true,
        }),
        SessionStatus::Errored => Some(StatusPill {
            label: "Failed",
            dot: "bg-destructive",
            pulse: false,
        }),
        SessionStatus::Idle | SessionStatus::Cancelled => None,
    }
}

/// Deterministic hash→HSL for stable per-identity colors
/// (CodexMonitor's subagent pills). Returns (hue, sat%, light%).
// Not yet referenced by the timeline (the subagent pills render
// plain); kept — unit-tested below — for that wiring.
#[allow(dead_code)]
pub fn hash_hsl(identity: &str) -> (u32, u32, u32) {
    let mut h: u32 = 2166136261;
    for b in identity.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16777619);
    }
    (h % 360, 55 + (h / 360) % 20, 45 + (h / 7200) % 15)
}

/// Free-context percentage for the ring; `None` when the window is
/// unknown (UI falls back to a raw counter).
pub fn context_free_percent(used_tokens: u64, context_window: u64) -> Option<f64> {
    if context_window == 0 {
        return None;
    }
    let used = (used_tokens as f64 / context_window as f64 * 100.0).min(100.0);
    Some(100.0 - used)
}

/// Inline style for the CSS-only context ring (CodexMonitor's
/// conic-gradient trick: one percentage drives both the arc sweep
/// and the red→green hue).
pub fn context_ring_style(free_percent: f64) -> String {
    let hue = 120.0 * free_percent / 100.0;
    format!(
        "background: radial-gradient(closest-side, var(--card, #111) 60%, transparent 61%), \
         conic-gradient(hsl({hue:.0} 70% 45%) {free_percent:.1}%, \
         color-mix(in srgb, currentColor 15%, transparent) 0);"
    )
}

/// Group models by provider for the picker's brand rail
/// (hermes-desktop's `groupModelsByProvider`): stable provider
/// order = first appearance, models sorted with the ACTIVE model
/// floated first (3-tier rank: active → default → rest, stable).
pub fn group_models(models: &[ModelInfo], current: &str) -> Vec<(String, String, Vec<ModelInfo>)> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut by_provider: std::collections::HashMap<String, Vec<ModelInfo>> =
        std::collections::HashMap::new();
    for m in models {
        if !order.iter().any(|(id, _)| *id == m.provider_id) {
            let name = if m.provider_name.is_empty() {
                m.provider_id.clone()
            } else {
                m.provider_name.clone()
            };
            order.push((m.provider_id.clone(), name));
        }
        by_provider
            .entry(m.provider_id.clone())
            .or_default()
            .push(m.clone());
    }
    order
        .into_iter()
        .map(|(id, name)| {
            let mut ms = by_provider.remove(&id).unwrap_or_default();
            ms.sort_by_key(|m| {
                if m.id == current {
                    0u8
                } else if m.is_default {
                    1
                } else {
                    2
                }
            });
            (id, name, ms)
        })
        .collect()
}

/// Compact cost badge: "$0.25/$2" per Mtok, or None when unknown.
pub fn cost_badge(cost_in: f64, cost_out: f64) -> Option<String> {
    if cost_in <= 0.0 && cost_out <= 0.0 {
        return None;
    }
    fn c(v: f64) -> String {
        if v >= 10.0 {
            format!("${v:.0}")
        } else {
            format!("${v:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        }
    }
    Some(format!("{}/{}", c(cost_in), c(cost_out)))
}

/// How close to the bottom (px) still counts as "following the tail".
const STICK_THRESHOLD_PX: u32 = 120;

/// Autoscroll script for the transcript container.
///
/// Two rules, because they conflict: jump to the bottom the first
/// time the list renders (you want the newest message), but after
/// that only follow the tail if the reader is already near it —
/// otherwise scrolling up to re-read something is undone by the next
/// streamed token. `dataset.init` marks the first pass, so this needs
/// no state on the Rust side.
#[must_use]
pub fn autoscroll_js(element_id: &str) -> String {
    format!(
        "(() => {{ const el = document.getElementById('{element_id}'); if (!el) return; \
         const gap = el.scrollHeight - el.scrollTop - el.clientHeight; \
         if (el.dataset.init !== '1' || gap < {STICK_THRESHOLD_PX}) {{ \
         el.scrollTop = el.scrollHeight; el.dataset.init = '1'; }} }})();"
    )
}

/// Where the reader is in the transcript (t3code's
/// `TimelineScrollMode`, minus the virtualized-list anchoring we
/// don't have the measurements for).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollMode {
    /// At (or near) the bottom — new output should scroll into view.
    FollowingEnd,
    /// Scrolled away to read something; leave the viewport alone and
    /// offer a way back.
    FreeScrolling,
}

/// Classify a scroll position. Same threshold the autoscroll script
/// uses, so the Jump-to-latest button appears exactly when following
/// stops.
#[must_use]
pub fn scroll_mode(scroll_top: f64, scroll_height: f64, client_height: f64) -> ScrollMode {
    let gap = scroll_height - scroll_top - client_height;
    if gap < f64::from(STICK_THRESHOLD_PX) {
        ScrollMode::FollowingEnd
    } else {
        ScrollMode::FreeScrolling
    }
}

/// Scroll the transcript to the bottom, unconditionally (the
/// Jump-to-latest button).
#[must_use]
pub fn scroll_to_end_js(element_id: &str) -> String {
    format!(
        "(() => {{ const el = document.getElementById('{element_id}'); \
         if (el) {{ el.scrollTop = el.scrollHeight; el.dataset.init = '1'; }} }})();"
    )
}

/// How many prompts to keep per conversation (CodexMonitor's cap).
const PROMPT_HISTORY_LIMIT: usize = 200;

/// Which way `↑`/`↓` walks the prompt history.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recall {
    Older,
    Newer,
}

/// Shell-style prompt recall for the composer (CodexMonitor's
/// `usePromptHistory`).
///
/// The rules exist so recall never fights ordinary editing: `↑` only
/// enters history from an *empty* composer (otherwise it's cursor
/// movement), `↓` does nothing until you're already navigating, and
/// whatever you had typed is restored when you walk back past the
/// newest entry.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct PromptHistory {
    entries: Vec<String>,
    /// Position while navigating; `None` = not in history.
    cursor: Option<usize>,
    /// What was in the composer before navigation started.
    draft: String,
}

impl PromptHistory {
    #[must_use]
    pub fn from_entries(entries: Vec<String>) -> Self {
        let mut me = Self::default();
        for e in entries {
            me.record(&e);
        }
        me
    }

    #[must_use]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Remember a sent prompt. Blank and immediate-repeat sends are
    /// dropped — resending the same thing twice shouldn't cost two
    /// presses of `↑` to get past.
    pub fn record(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() || self.entries.last().map(String::as_str) == Some(trimmed) {
            self.cursor = None;
            return;
        }
        self.entries.push(trimmed.to_string());
        let overflow = self.entries.len().saturating_sub(PROMPT_HISTORY_LIMIT);
        if overflow > 0 {
            self.entries.drain(..overflow);
        }
        self.cursor = None;
    }

    /// Leave history without changing the composer — call when the
    /// user types.
    pub fn reset(&mut self) {
        self.cursor = None;
    }

    /// Step through history. `Some(text)` = put this in the composer
    /// and consume the key; `None` = not ours, let the key through.
    pub fn recall(&mut self, dir: Recall, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match (self.cursor, dir) {
            // Entering history: only from an empty composer, and only
            // going back.
            (None, Recall::Newer) => None,
            (None, Recall::Older) => {
                if !current.trim().is_empty() {
                    return None;
                }
                self.draft = current.to_string();
                let idx = self.entries.len() - 1;
                self.cursor = Some(idx);
                Some(self.entries[idx].clone())
            }
            (Some(i), Recall::Older) => {
                let next = i.saturating_sub(1);
                self.cursor = Some(next);
                Some(self.entries[next].clone())
            }
            (Some(i), Recall::Newer) => {
                if i + 1 >= self.entries.len() {
                    // Past the newest: back to what you were writing.
                    self.cursor = None;
                    return Some(std::mem::take(&mut self.draft));
                }
                self.cursor = Some(i + 1);
                Some(self.entries[i + 1].clone())
            }
        }
    }
}

/// Vault paths an assistant message refers to (CodexMonitor's
/// `messageFileLinks`, narrowed to our domain).
///
/// Now that the agent reads and writes notes it cites paths
/// constantly — `Records/notes/standup.md`. Surfacing them as chips
/// under the message turns a dead string into one click, without
/// touching the markdown pipeline. Deduped, order preserved, capped.
#[must_use]
pub fn referenced_paths(text: &str) -> Vec<String> {
    const MAX: usize = 8;
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '"')) {
        // Markdown and prose cling to paths: `foo.md`, "foo.md",
        // (foo.md), foo.md. — strip the decoration, keep the path.
        let candidate = raw.trim_matches(|c: char| matches!(c, '`' | '\'' | '[' | ']' | '*' | '_'));
        let candidate = candidate.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if !candidate.ends_with(".md") || candidate.len() <= 3 {
            continue;
        }
        // Vault-relative only: no URLs, no absolute paths, no
        // parent-dir escapes.
        if candidate.contains("://") || candidate.starts_with('/') || candidate.contains("..") {
            continue;
        }
        if !out.iter().any(|p| p == candidate) {
            out.push(candidate.to_string());
        }
        if out.len() >= MAX {
            break;
        }
    }
    out
}

pub fn fmt_elapsed(secs: i64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Tool-call wall time: sub-second in ms, then seconds, then m/s.
pub fn fmt_duration(ms: u32) -> String {
    match ms {
        0..=999 => format!("{ms}ms"),
        1000..=59_999 => format!("{:.1}s", f64::from(ms) / 1000.0),
        _ => {
            let secs = ms / 1000;
            format!("{}m{:02}s", secs / 60, secs % 60)
        }
    }
}

/// Accrued spend for a session. Sub-cent turns still deserve a
/// number — "$0.00" reads as free, which it isn't.
pub fn fmt_cost(usd: f32) -> Option<String> {
    if usd <= 0.0 {
        return None;
    }
    if usd < 0.01 {
        return Some(format!("${usd:.4}"));
    }
    Some(format!("${usd:.2}"))
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// "44m" / "11h" / "4d" style relative timestamps for the rail.
pub fn relative_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_orders_exact_prefix_substring_subsequence() {
        assert_eq!(score_match("learn", "learn"), Some(110));
        assert_eq!(score_match("lea", "learn"), Some(95));
        assert_eq!(score_match("arn", "learn"), Some(70));
        assert_eq!(score_match("lrn", "learn"), Some(50));
        assert_eq!(score_match("xyz", "learn"), None);
    }

    #[test]
    fn rank_by_sorts_and_caps() {
        let items = vec!["learn", "clean", "lean-startup", "unrelated"];
        let ranked = rank_by(&items, "lean", |s| (*s).to_string(), 2);
        // "lean-startup" is a prefix match (95); "clean" substring (70);
        // "learn" subsequence (50); "unrelated" none.
        assert_eq!(ranked, vec!["lean-startup", "clean"]);
    }

    #[test]
    fn hash_hsl_is_stable_and_in_range() {
        let a = hash_hsl("researcher");
        assert_eq!(a, hash_hsl("researcher"));
        assert!(a.0 < 360 && a.1 <= 75 && a.2 <= 60);
        assert_ne!(a, hash_hsl("curator"));
    }

    fn mi(provider: &str, id: &str, default: bool) -> ModelInfo {
        ModelInfo {
            backend_id: "hermes".into(),
            id: id.into(),
            label: String::new(),
            is_default: default,
            context_length: 0,
            provider_id: provider.into(),
            provider_name: provider.to_uppercase(),
            reasoning: false,
            cost_in_per_mtok: 0.0,
            cost_out_per_mtok: 0.0,
        }
    }

    #[test]
    fn group_models_keeps_order_and_floats_active() {
        let models = vec![
            mi("hermes", "hermes", true),
            mi("openai", "openai/gpt-a", false),
            mi("openai", "openai/gpt-b", false),
            mi("anthropic", "anthropic/claude", false),
        ];
        let groups = group_models(&models, "openai/gpt-b");
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, "hermes");
        assert_eq!(groups[1].0, "openai");
        // Active model floats first within its provider.
        assert_eq!(groups[1].2[0].id, "openai/gpt-b");
        assert_eq!(groups[2].1, "ANTHROPIC");
    }

    #[test]
    fn cost_badge_formats_and_hides_unknown() {
        assert_eq!(cost_badge(0.0, 0.0), None);
        assert_eq!(cost_badge(0.25, 2.0), Some("$0.25/$2".into()));
        assert_eq!(cost_badge(15.0, 75.0), Some("$15/$75".into()));
    }

    #[test]
    fn autoscroll_targets_the_element_and_respects_the_stick_threshold() {
        let js = autoscroll_js("agent-transcript-sess-42");
        assert!(
            js.contains("getElementById('agent-transcript-sess-42')"),
            "{js}"
        );
        // Both rules must be present: the first-render jump and the
        // near-the-bottom check that keeps a scrolled-up reader put.
        assert!(js.contains("dataset.init"), "{js}");
        assert!(js.contains(&STICK_THRESHOLD_PX.to_string()), "{js}");
        assert!(
            js.contains("scrollHeight - el.scrollTop - el.clientHeight"),
            "{js}"
        );
    }

    #[test]
    fn scroll_mode_flips_at_the_stick_threshold() {
        // Pinned to the bottom.
        assert_eq!(scroll_mode(900.0, 1000.0, 100.0), ScrollMode::FollowingEnd);
        // Just inside the threshold still counts as following.
        assert_eq!(scroll_mode(810.0, 1000.0, 100.0), ScrollMode::FollowingEnd);
        // Scrolled up to read.
        assert_eq!(scroll_mode(200.0, 1000.0, 100.0), ScrollMode::FreeScrolling);
        // Content shorter than the viewport can't be scrolled away from.
        assert_eq!(scroll_mode(0.0, 50.0, 400.0), ScrollMode::FollowingEnd);
    }

    #[test]
    fn history_enters_only_from_an_empty_composer() {
        let mut h = PromptHistory::from_entries(vec!["first".into(), "second".into()]);
        // Mid-edit `↑` is cursor movement, not recall.
        assert_eq!(h.recall(Recall::Older, "half-typed"), None);
        // `↓` does nothing until you're navigating.
        assert_eq!(h.recall(Recall::Newer, ""), None);
        assert_eq!(h.recall(Recall::Older, "").as_deref(), Some("second"));
        assert_eq!(h.recall(Recall::Older, "second").as_deref(), Some("first"));
        // Clamps at the oldest instead of wrapping.
        assert_eq!(h.recall(Recall::Older, "first").as_deref(), Some("first"));
    }

    #[test]
    fn history_restores_the_draft_on_the_way_back() {
        let mut h = PromptHistory::from_entries(vec!["older".into(), "newer".into()]);
        // A draft is only preserved if there was one; entering
        // requires empty, so the realistic case is an empty draft.
        assert_eq!(h.recall(Recall::Older, "").as_deref(), Some("newer"));
        assert_eq!(h.recall(Recall::Older, "newer").as_deref(), Some("older"));
        assert_eq!(h.recall(Recall::Newer, "older").as_deref(), Some("newer"));
        // Past the newest → back to the (empty) draft, not stuck.
        assert_eq!(h.recall(Recall::Newer, "newer").as_deref(), Some(""));
        // And we've left history, so `↓` is inert again.
        assert_eq!(h.recall(Recall::Newer, ""), None);
    }

    #[test]
    fn history_dedupes_repeats_and_caps() {
        let mut h = PromptHistory::default();
        h.record("  hi  ");
        h.record("hi");
        h.record("");
        assert_eq!(h.entries(), ["hi"]);
        for i in 0..PROMPT_HISTORY_LIMIT + 20 {
            h.record(&format!("p{i}"));
        }
        assert_eq!(h.entries().len(), PROMPT_HISTORY_LIMIT);
        // Oldest dropped, newest kept.
        assert_eq!(h.entries().last().unwrap(), "p219");
    }

    #[test]
    fn recording_leaves_history_navigation() {
        let mut h = PromptHistory::from_entries(vec!["a".into()]);
        assert!(h.recall(Recall::Older, "").is_some());
        h.record("b");
        // Sending resets the cursor, so the next `↑` starts fresh.
        assert_eq!(h.recall(Recall::Older, "").as_deref(), Some("b"));
    }

    #[test]
    fn referenced_paths_finds_vault_notes_in_prose() {
        let text = "I read `Records/notes/standup.md` and updated Projects/site.md. \
                    See [the note](Wiki/Knowledge/mcp.md) too.";
        assert_eq!(
            referenced_paths(text),
            vec![
                "Records/notes/standup.md".to_string(),
                "Projects/site.md".into(),
                "Wiki/Knowledge/mcp.md".into(),
            ]
        );
    }

    #[test]
    fn referenced_paths_rejects_what_isnt_a_vault_note() {
        // URLs, absolute paths and traversal are not vault-relative.
        assert!(referenced_paths("see https://example.com/a.md").is_empty());
        assert!(referenced_paths("/etc/notes/x.md").is_empty());
        assert!(referenced_paths("../outside/x.md").is_empty());
        // Non-markdown and bare extensions don't qualify.
        assert!(referenced_paths("src/main.rs and .md").is_empty());
    }

    #[test]
    fn referenced_paths_dedupes_and_caps() {
        let repeated = "a/x.md a/x.md a/x.md";
        assert_eq!(referenced_paths(repeated), vec!["a/x.md".to_string()]);
        let many: String = (0..20).map(|i| format!("d/n{i}.md ")).collect();
        assert_eq!(referenced_paths(&many).len(), 8);
    }

    #[test]
    fn fmt_duration_switches_units() {
        assert_eq!(fmt_duration(0), "0ms");
        assert_eq!(fmt_duration(842), "842ms");
        assert_eq!(fmt_duration(1500), "1.5s");
        assert_eq!(fmt_duration(75_000), "1m15s");
    }

    #[test]
    fn fmt_cost_keeps_sub_cent_spend_visible() {
        assert_eq!(fmt_cost(0.0), None);
        assert_eq!(fmt_cost(0.0004), Some("$0.0004".into()));
        assert_eq!(fmt_cost(1.239), Some("$1.24".into()));
    }

    #[test]
    fn context_percent_handles_unknown_and_overflow() {
        assert_eq!(context_free_percent(500, 0), None);
        assert_eq!(context_free_percent(250, 1000), Some(75.0));
        assert_eq!(context_free_percent(5000, 1000), Some(0.0));
    }
}
