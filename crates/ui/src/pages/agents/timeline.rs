//! The transcript's pure row model — t3code's
//! `MessagesTimeline.logic.ts` as Rust: a discriminated union of
//! row kinds derived by plain functions, so the view is one
//! `match` and the folding/grouping behavior is unit-tested.

use agent_proto::message::{Message, Role};

/// Tone of one activity line (tool event / warning), driving the
/// status glyph: `Running` ▸, `Ok` ✓, `Fail` ✗, `Note` ·.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToolTone {
    Running,
    Ok,
    Fail,
    #[default]
    Note,
}

/// One line in a turn's activity log.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ActivityLine {
    pub tone: ToolTone,
    pub text: String,
    /// Tool-call id when known — lets `ToolFinished` settle the
    /// matching `Running` line instead of appending a duplicate.
    pub tool_id: String,
    /// Pretty-printed call arguments, shown when the row is
    /// expanded. Empty for non-tool lines.
    pub args: String,
    /// Tool result preview, filled in when the call settles.
    pub output: String,
    /// Wall time of the call once settled; `0` while running.
    pub duration_ms: u32,
}

impl ActivityLine {
    /// A plain note — a warning, a status hint, a cancellation.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            tone: ToolTone::Note,
            text: text.into(),
            ..Default::default()
        }
    }

    /// Whether the row has anything worth expanding to.
    pub fn has_detail(&self) -> bool {
        !self.args.is_empty() || !self.output.is_empty()
    }
}

/// The retained work log of one completed turn, anchored to the
/// assistant message that concluded it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TurnLog {
    pub lines: Vec<ActivityLine>,
    pub reasoning: String,
    pub duration_secs: i64,
}

/// One transcript row. `Message` carries an index into the
/// caller's message list; `TurnFold` anchors a completed turn's
/// work log *before* its assistant message (t3code's turn-fold).
#[derive(Clone, PartialEq, Debug)]
pub enum Row {
    Message(usize),
    TurnFold {
        /// Anchor assistant-message id (the `TurnLog` key).
        anchor: String,
        /// Pre-rendered one-line summary ("3 tool calls · 0:42").
        summary: String,
    },
}

/// Derive the settled transcript rows: each assistant message that
/// has a retained `TurnLog` gets a fold row directly above it.
pub fn build_rows(
    messages: &[Message],
    turns: &std::collections::HashMap<String, TurnLog>,
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(messages.len() + turns.len());
    for (i, m) in messages.iter().enumerate() {
        if matches!(m.role, Role::Assistant) {
            if let Some(log) = turns.get(&m.id) {
                if !log.lines.is_empty() || !log.reasoning.is_empty() {
                    rows.push(Row::TurnFold {
                        anchor: m.id.clone(),
                        summary: fold_summary(&log.lines, log.duration_secs),
                    });
                }
            }
        }
        rows.push(Row::Message(i));
    }
    rows
}

/// Append a live activity line, deduping immediate repeats and
/// capping the list (drops oldest).
pub fn push_line(lines: &mut Vec<ActivityLine>, line: ActivityLine) {
    if lines
        .last()
        .is_some_and(|l| l.text == line.text && l.tone == line.tone)
    {
        return;
    }
    lines.push(line);
    let overflow = lines.len().saturating_sub(200);
    if overflow > 0 {
        lines.drain(..overflow);
    }
}

/// Settle a `Running` line when its tool finishes: match by tool id
/// (or, failing that, the most recent `Running` line), set the tone
/// and fold in the result + duration. Appends a fresh line when no
/// running line matches (finish-without-start).
pub fn settle_tool(
    lines: &mut Vec<ActivityLine>,
    tool_id: &str,
    text: String,
    ok: bool,
    duration_ms: u32,
    output: String,
) {
    let tone = if ok { ToolTone::Ok } else { ToolTone::Fail };
    let target = lines
        .iter()
        .rposition(|l| l.tone == ToolTone::Running && !tool_id.is_empty() && l.tool_id == tool_id)
        .or_else(|| lines.iter().rposition(|l| l.tone == ToolTone::Running));
    match target {
        Some(i) => {
            lines[i].tone = tone;
            lines[i].text = text;
            lines[i].duration_ms = duration_ms;
            lines[i].output = output;
        }
        None => push_line(
            lines,
            ActivityLine {
                tone,
                text,
                tool_id: tool_id.to_string(),
                duration_ms,
                output,
                args: String::new(),
            },
        ),
    }
}

/// The tool call still in flight, if any — drives the Working row's
/// "⚙ terminal · cargo test" line.
pub fn running_tool(lines: &[ActivityLine]) -> Option<&ActivityLine> {
    lines.iter().rev().find(|l| l.tone == ToolTone::Running)
}

/// Tool calls in a log (notes and warnings don't count) and how many
/// of them failed.
pub fn tool_stats(lines: &[ActivityLine]) -> (usize, usize) {
    let calls = lines.iter().filter(|l| l.tone != ToolTone::Note);
    let total = calls.clone().count();
    (total, calls.filter(|l| l.tone == ToolTone::Fail).count())
}

/// The one-line fold summary: "3 tool calls · 1 failed · 0:42".
pub fn fold_summary(lines: &[ActivityLine], duration_secs: i64) -> String {
    let (total, failed) = tool_stats(lines);
    let mut out = if total == 0 {
        // A turn can log only notes (a warning, a cancellation).
        "activity".to_string()
    } else {
        format!("{total} tool call{}", if total == 1 { "" } else { "s" })
    };
    if failed > 0 {
        out.push_str(&format!(" · {failed} failed"));
    }
    if duration_secs > 0 {
        out.push_str(&format!(" · {}", super::logic::fmt_elapsed(duration_secs)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::message::ContentBlock;
    use chrono::Utc;

    fn msg(id: &str, role: Role) -> Message {
        Message {
            id: id.into(),
            session_id: "s".into(),
            role,
            content: vec![ContentBlock::Text { text: "t".into() }],
            partial: false,
            errored: false,
            error_text: String::new(),
            reasoning: None,
            created_at: Utc::now(),
        }
    }

    fn line(tone: ToolTone, text: &str, id: &str) -> ActivityLine {
        ActivityLine {
            tone,
            text: text.into(),
            tool_id: id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn fold_lands_before_its_assistant_message_only() {
        let messages = vec![
            msg("u1", Role::User),
            msg("a1", Role::Assistant),
            msg("u2", Role::User),
            msg("a2", Role::Assistant),
        ];
        let mut turns = std::collections::HashMap::new();
        turns.insert(
            "a2".to_string(),
            TurnLog {
                lines: vec![line(ToolTone::Ok, "grep", "t1")],
                reasoning: String::new(),
                duration_secs: 42,
            },
        );
        let rows = build_rows(&messages, &turns);
        assert_eq!(
            rows,
            vec![
                Row::Message(0),
                Row::Message(1),
                Row::Message(2),
                Row::TurnFold {
                    anchor: "a2".into(),
                    summary: "1 tool call · 0:42".into(),
                },
                Row::Message(3),
            ]
        );
    }

    #[test]
    fn empty_turn_logs_produce_no_fold() {
        let messages = vec![msg("a1", Role::Assistant)];
        let mut turns = std::collections::HashMap::new();
        turns.insert("a1".to_string(), TurnLog::default());
        assert_eq!(build_rows(&messages, &turns), vec![Row::Message(0)]);
    }

    #[test]
    fn push_line_dedupes_and_caps() {
        let mut lines = Vec::new();
        push_line(&mut lines, line(ToolTone::Note, "x", ""));
        push_line(&mut lines, line(ToolTone::Note, "x", ""));
        assert_eq!(lines.len(), 1);
        for i in 0..250 {
            push_line(&mut lines, line(ToolTone::Note, &format!("l{i}"), ""));
        }
        assert_eq!(lines.len(), 200);
    }

    #[test]
    fn settle_matches_by_id_then_falls_back_to_last_running() {
        let mut lines = vec![
            line(ToolTone::Running, "run a", "a"),
            line(ToolTone::Running, "run b", "b"),
        ];
        settle_tool(&mut lines, "a", "ran a".into(), true, 1500, "out".into());
        assert_eq!(lines[0].tone, ToolTone::Ok);
        assert_eq!(lines[0].duration_ms, 1500);
        assert_eq!(lines[0].output, "out");
        settle_tool(&mut lines, "", "ran b".into(), false, 0, String::new());
        assert_eq!(lines[1].tone, ToolTone::Fail);
        // Finish-without-start appends.
        settle_tool(&mut lines, "c", "ran c".into(), true, 0, String::new());
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn running_tool_finds_the_newest_in_flight_call() {
        let lines = vec![
            line(ToolTone::Ok, "done", "a"),
            line(ToolTone::Running, "in flight", "b"),
            line(ToolTone::Note, "a note", ""),
        ];
        assert_eq!(running_tool(&lines).unwrap().text, "in flight");
        assert!(running_tool(&[line(ToolTone::Ok, "done", "a")]).is_none());
    }

    #[test]
    fn fold_summary_counts_calls_not_notes_and_flags_failures() {
        let lines = vec![
            line(ToolTone::Ok, "a", "1"),
            line(ToolTone::Fail, "b", "2"),
            line(ToolTone::Note, "just a warning", ""),
        ];
        assert_eq!(fold_summary(&lines, 42), "2 tool calls · 1 failed · 0:42");
        assert_eq!(
            fold_summary(&[line(ToolTone::Ok, "a", "1")], 0),
            "1 tool call"
        );
        // Notes only — still worth a fold, but not "0 tool calls".
        assert_eq!(
            fold_summary(&[line(ToolTone::Note, "cancelled", "")], 0),
            "activity"
        );
    }

    #[test]
    fn has_detail_gates_the_expander() {
        assert!(!ActivityLine::note("hi").has_detail());
        let mut l = line(ToolTone::Ok, "t", "1");
        l.output = "result".into();
        assert!(l.has_detail());
    }
}
