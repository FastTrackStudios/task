//! JWZ thread reconstruction.
//!
//! Algorithm by Jamie Zawinski, used by every major mail client.
//! Given a set of messages with `(message_id, in_reply_to,
//! references)` triples, computes one **thread root** per
//! connected component so the UI can render threads.
//!
//! High-level steps (per the original spec):
//! 1. Build a `Container` per Message-ID, including any IDs
//!    that appear in `References` but never as a real message.
//! 2. Link each message to its parent via the most-immediate
//!    reference (last entry in `References`, or `In-Reply-To`
//!    if `References` is absent).
//! 3. Walk every connected component; the root is the container
//!    that has no parent. Every message in the component gets
//!    `thread_id = root.message_id`.
//!
//! We skip JWZ's optional "merge by normalized subject" step
//! (steps 5b/5c in the original) for now — it's the source of
//! ~most threading complaints when wrong, and not worth the
//! false-positive risk in v1. Can be re-added behind a config
//! flag.
//!
//! Pure: no IO, no allocations beyond the working set. Take a
//! slice of triples, return a `Vec<(message_id, thread_id)>`
//! assignment table the caller persists.

use std::collections::{HashMap, HashSet};

/// One input row to the threading algorithm. `references` is
/// ordered oldest → newest as it appears in the RFC2822
/// References header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadInput<'a> {
    pub message_id: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [&'a str],
}

/// One output row: `message_id` → `thread_id`. `thread_id` is the
/// Message-ID of the thread root (which may be the message
/// itself for a single-message thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAssignment {
    pub message_id: String,
    pub thread_id: String,
}

/// Run the JWZ pass over `inputs`. Returns one assignment per
/// input row, plus implicit IDs are dropped — only IDs we
/// actually have a message for are returned.
#[must_use]
pub fn compute_threads(inputs: &[ThreadInput<'_>]) -> Vec<ThreadAssignment> {
    // 1. Build the container set. Real messages first, then
    //    referenced-but-missing IDs as placeholders.
    let mut parent: HashMap<String, Option<String>> = HashMap::new();
    let real: HashSet<String> = inputs.iter().map(|i| i.message_id.to_string()).collect();

    for input in inputs {
        // Make sure the message itself is in the graph even if
        // it has no references.
        parent.entry(input.message_id.to_string()).or_insert(None);

        // 2. Walk the References chain front-to-back, linking
        //    each ref to the next as parent. Then link the
        //    message itself to the *last* reference (its
        //    immediate parent).
        let refs = input.references;
        if refs.len() >= 2 {
            for pair in refs.windows(2) {
                let p = pair[0].to_string();
                let c = pair[1].to_string();
                parent.entry(p.clone()).or_insert(None);
                let entry = parent.entry(c).or_insert(None);
                // Only set parent if we don't already know
                // something better — but the JWZ spec says
                // links can overwrite if the new one is more
                // specific. For our purposes, the first link
                // we see wins (deterministic + cheap).
                if entry.is_none() {
                    *entry = Some(p);
                }
            }
        }
        // Direct parent: last reference, or In-Reply-To, or
        // neither (orphan / root).
        let direct_parent: Option<String> = refs
            .last()
            .map(std::string::ToString::to_string)
            .or_else(|| input.in_reply_to.map(std::string::ToString::to_string));
        if let Some(p) = direct_parent {
            parent.entry(p.clone()).or_insert(None);
            let entry = parent.entry(input.message_id.to_string()).or_insert(None);
            if entry.is_none() {
                *entry = Some(p);
            }
        }
    }

    // 3. For each real message, walk up to root via union-find
    //    style memoization.
    let mut root_of: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(inputs.len());
    for input in inputs {
        let mid = input.message_id.to_string();
        let root = find_root(&parent, &mut root_of, &mid);
        out.push(ThreadAssignment {
            message_id: mid,
            thread_id: root,
        });
    }
    let _ = real;
    out
}

/// Walk `parent` upward from `start` until we hit a node with
/// no parent. Memoizes intermediate results so the next walk
/// short-circuits. Cycle-safe — if we ever revisit a node
/// mid-walk, we treat the current as the root.
fn find_root(
    parent: &HashMap<String, Option<String>>,
    cache: &mut HashMap<String, String>,
    start: &str,
) -> String {
    if let Some(cached) = cache.get(start) {
        return cached.clone();
    }
    let mut visited = HashSet::new();
    let mut cur = start.to_string();
    loop {
        if !visited.insert(cur.clone()) {
            // Cycle detected — break by treating `cur` as root.
            // Should never happen with well-formed input but
            // RFC2822 doesn't enforce DAG shape on References.
            break;
        }
        match parent.get(&cur).cloned() {
            Some(Some(p)) => cur = p,
            _ => break,
        }
    }
    // Memoize every node we passed through so the next find
    // short-circuits.
    for v in &visited {
        cache.insert(v.clone(), cur.clone());
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &'static str) -> &'static str {
        s
    }

    #[test]
    fn single_message_threads_to_itself() {
        let inputs = vec![ThreadInput {
            message_id: "<a>",
            in_reply_to: None,
            references: &[],
        }];
        let out = compute_threads(&inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].thread_id, "<a>");
    }

    #[test]
    fn reply_threads_to_parent() {
        let refs = [id("<a>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<a>",
                in_reply_to: None,
                references: &[],
            },
            ThreadInput {
                message_id: "<b>",
                in_reply_to: Some("<a>"),
                references: &refs,
            },
        ];
        let out = compute_threads(&inputs);
        let map: HashMap<_, _> = out
            .iter()
            .map(|a| (a.message_id.clone(), a.thread_id.clone()))
            .collect();
        assert_eq!(map["<a>"], "<a>");
        assert_eq!(map["<b>"], "<a>");
    }

    #[test]
    fn deep_chain_threads_to_root() {
        let refs_b = [id("<a>")];
        let refs_c = [id("<a>"), id("<b>")];
        let refs_d = [id("<a>"), id("<b>"), id("<c>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<a>",
                in_reply_to: None,
                references: &[],
            },
            ThreadInput {
                message_id: "<b>",
                in_reply_to: Some("<a>"),
                references: &refs_b,
            },
            ThreadInput {
                message_id: "<c>",
                in_reply_to: Some("<b>"),
                references: &refs_c,
            },
            ThreadInput {
                message_id: "<d>",
                in_reply_to: Some("<c>"),
                references: &refs_d,
            },
        ];
        let out = compute_threads(&inputs);
        for assignment in out {
            assert_eq!(
                assignment.thread_id, "<a>",
                "expected {} → <a>; got {}",
                assignment.message_id, assignment.thread_id
            );
        }
    }

    #[test]
    fn two_separate_threads_stay_separate() {
        let refs = [id("<a>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<a>",
                in_reply_to: None,
                references: &[],
            },
            ThreadInput {
                message_id: "<b>",
                in_reply_to: Some("<a>"),
                references: &refs,
            },
            ThreadInput {
                message_id: "<x>",
                in_reply_to: None,
                references: &[],
            },
        ];
        let out = compute_threads(&inputs);
        let map: HashMap<_, _> = out
            .iter()
            .map(|a| (a.message_id.clone(), a.thread_id.clone()))
            .collect();
        assert_eq!(map["<a>"], "<a>");
        assert_eq!(map["<b>"], "<a>");
        assert_eq!(map["<x>"], "<x>");
    }

    #[test]
    fn orphan_with_unknown_parent_is_its_own_thread() {
        // <b> claims to reply to <missing>, which we never see.
        // JWZ assigns <missing> as the apparent root, but since
        // we only emit assignments for inputs we have, the
        // result is: <b> → <missing> (a phantom root the UI
        // can render as "[Unknown parent]" or fold into <b>).
        let refs = [id("<missing>")];
        let inputs = vec![ThreadInput {
            message_id: "<b>",
            in_reply_to: Some("<missing>"),
            references: &refs,
        }];
        let out = compute_threads(&inputs);
        assert_eq!(out[0].thread_id, "<missing>");
    }

    #[test]
    fn cycle_in_references_does_not_loop_forever() {
        // <a> claims <b> as parent, <b> claims <a>. Should
        // converge.
        let refs_a = [id("<b>")];
        let refs_b = [id("<a>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<a>",
                in_reply_to: Some("<b>"),
                references: &refs_a,
            },
            ThreadInput {
                message_id: "<b>",
                in_reply_to: Some("<a>"),
                references: &refs_b,
            },
        ];
        // The test passes iff this returns (no infinite loop).
        let out = compute_threads(&inputs);
        assert_eq!(out.len(), 2);
        // Both should land on the SAME thread_id (whichever
        // node breaks the cycle first).
        assert_eq!(out[0].thread_id, out[1].thread_id);
    }

    #[test]
    fn fan_out_replies_all_share_root() {
        // <a> root; <b>, <c>, <d> all reply to <a>.
        let refs = [id("<a>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<a>",
                in_reply_to: None,
                references: &[],
            },
            ThreadInput {
                message_id: "<b>",
                in_reply_to: Some("<a>"),
                references: &refs,
            },
            ThreadInput {
                message_id: "<c>",
                in_reply_to: Some("<a>"),
                references: &refs,
            },
            ThreadInput {
                message_id: "<d>",
                in_reply_to: Some("<a>"),
                references: &refs,
            },
        ];
        let out = compute_threads(&inputs);
        for a in &out {
            assert_eq!(a.thread_id, "<a>");
        }
    }

    #[test]
    fn references_take_precedence_over_in_reply_to() {
        // Both headers present: References' last entry wins
        // (per RFC 5256 + JWZ note).
        let refs_leaf = [id("<root>"), id("<mid>")];
        let refs_mid = [id("<root>")];
        let inputs = vec![
            ThreadInput {
                message_id: "<root>",
                in_reply_to: None,
                references: &[],
            },
            ThreadInput {
                message_id: "<mid>",
                in_reply_to: Some("<root>"),
                references: &refs_mid,
            },
            // Leaf claims `<other>` as in-reply-to but the
            // References chain says `<mid>` is the immediate
            // parent.
            ThreadInput {
                message_id: "<leaf>",
                in_reply_to: Some("<other>"),
                references: &refs_leaf,
            },
        ];
        let out = compute_threads(&inputs);
        let map: HashMap<_, _> = out
            .iter()
            .map(|a| (a.message_id.clone(), a.thread_id.clone()))
            .collect();
        assert_eq!(map["<leaf>"], "<root>");
    }
}
