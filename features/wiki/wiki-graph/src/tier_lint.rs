//! Layered-access wikilink lint.
//!
//! Enforces the convention documented on
//! `org_proto::OrgRoot::wiki_dir`:
//!
//! - `vault/`           ← can link → `wiki/Knowledge`, `wiki/LLM`
//! - `wiki/LLM/`        ← can link → `wiki/Knowledge`
//! - `wiki/Knowledge/`  ← stays self-contained. No outbound
//!   links to `vault/` or `wiki/LLM/`.
//!
//! Pure walk; no LLM. For each markdown file in the
//! constrained tier ([`Tier::Knowledge`] or [`Tier::Llm`]),
//! extracts `[[Target]]` references and resolves each by
//! basename across the whole org tree. A target whose only
//! match lives in a forbidden tier is a [`TierViolation`].
//!
//! Broken links (target found in no tier) are *not* reported
//! here — that's the existing `wiki gaps` surface. This lint
//! is exclusively about cross-tier access.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::parse::extract_wikilinks;

/// Which tier a markdown file lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Personal data under `<org>/vault/`.
    Vault,
    /// Curated wiki at `<org>/wiki/Knowledge/`.
    Knowledge,
    /// LLM scratch at `<org>/wiki/LLM/`.
    Llm,
}

impl Tier {
    /// `true` if a page in `self` is allowed to link to a
    /// page in `target`.
    #[must_use]
    pub fn may_link_to(self, target: Tier) -> bool {
        match (self, target) {
            // Vault links freely into the wiki.
            (Tier::Vault, _) => true,
            // LLM links into Knowledge but not Vault.
            (Tier::Llm, Tier::Knowledge | Tier::Llm) => true,
            (Tier::Llm, Tier::Vault) => false,
            // Knowledge stays self-contained.
            (Tier::Knowledge, Tier::Knowledge) => true,
            (Tier::Knowledge, _) => false,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Vault => "vault",
            Tier::Knowledge => "wiki/Knowledge",
            Tier::Llm => "wiki/LLM",
        }
    }
}

/// One layered-access rule violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierViolation {
    /// Org-relative path of the page that contains the bad
    /// link (e.g. `wiki/Knowledge/Concepts/Foo.md`).
    pub source: PathBuf,
    /// Which tier the source page lives in.
    pub source_tier: Tier,
    /// The wikilink target text as written
    /// (`[[Target]]` → `"Target"`).
    pub target_link: String,
    /// Org-relative path the link resolves to in a
    /// forbidden tier.
    pub resolved: PathBuf,
    /// Which tier the resolved file lives in.
    pub resolved_tier: Tier,
}

/// Walk an org root and return every cross-tier wikilink
/// violation. `org_root` must contain `vault/`, `wiki/`, or
/// both — missing trees are simply skipped.
///
/// Resolution is by file basename (case-insensitive), matching
/// vanilla Obsidian behavior. Ambiguous targets (same name in
/// multiple tiers) prefer same-tier first, then `Knowledge`,
/// then `Llm`, then `Vault`. The lint surfaces a violation only
/// when the chosen match is in a forbidden tier.
#[must_use]
pub fn lint_org_tree(org_root: &Path) -> Vec<TierViolation> {
    let vault_root = org_root.join("vault");
    let knowledge_root = org_root.join("wiki").join("Knowledge");
    let llm_root = org_root.join("wiki").join("LLM");

    // Build a basename → (tier, rel_path) index across all
    // three trees. A single basename may exist in multiple
    // tiers; we collect all of them and pick at resolve time.
    let mut index: HashMap<String, Vec<(Tier, PathBuf)>> = HashMap::new();
    for (tier, root) in [
        (Tier::Knowledge, &knowledge_root),
        (Tier::Llm, &llm_root),
        (Tier::Vault, &vault_root),
    ] {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
            })
        {
            let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(rel) = entry.path().strip_prefix(org_root) else {
                continue;
            };
            index
                .entry(stem.to_lowercase())
                .or_default()
                .push((tier, rel.to_path_buf()));
        }
    }

    // For each source page in a constrained tier, scan links.
    let mut violations = Vec::new();
    for (source_tier, root) in [(Tier::Knowledge, &knowledge_root), (Tier::Llm, &llm_root)] {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
            })
        {
            let Ok(body) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(source_rel) = entry.path().strip_prefix(org_root) else {
                continue;
            };
            for target_link in extract_wikilinks(&body) {
                // Skip federated cross-org links — they go
                // through a different resolver and aren't a
                // tier violation.
                if target_link.starts_with('@') {
                    continue;
                }
                let key = target_link.to_lowercase();
                let Some(candidates) = index.get(&key) else {
                    continue;
                };
                let Some((tier, rel)) = pick_resolution(candidates, source_tier) else {
                    continue;
                };
                if !source_tier.may_link_to(*tier) {
                    violations.push(TierViolation {
                        source: source_rel.to_path_buf(),
                        source_tier,
                        target_link,
                        resolved: rel.clone(),
                        resolved_tier: *tier,
                    });
                }
            }
        }
    }

    violations.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.target_link.cmp(&b.target_link))
    });
    violations
}

/// Pick which candidate match a wikilink resolves to.
/// Priority: same tier as source > Knowledge > LLM > Vault.
/// Mirrors Obsidian's "prefer current folder" heuristic
/// while staying deterministic.
fn pick_resolution(candidates: &[(Tier, PathBuf)], source_tier: Tier) -> Option<&(Tier, PathBuf)> {
    let by_priority = |t: Tier| match t {
        _ if t == source_tier => 0,
        Tier::Knowledge => 1,
        Tier::Llm => 2,
        Tier::Vault => 3,
    };
    candidates.iter().min_by_key(|(t, _)| by_priority(*t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_links_match_matrix() {
        // Vault is permissive.
        assert!(Tier::Vault.may_link_to(Tier::Vault));
        assert!(Tier::Vault.may_link_to(Tier::Knowledge));
        assert!(Tier::Vault.may_link_to(Tier::Llm));

        // LLM avoids Vault.
        assert!(Tier::Llm.may_link_to(Tier::Knowledge));
        assert!(Tier::Llm.may_link_to(Tier::Llm));
        assert!(!Tier::Llm.may_link_to(Tier::Vault));

        // Knowledge stays in.
        assert!(Tier::Knowledge.may_link_to(Tier::Knowledge));
        assert!(!Tier::Knowledge.may_link_to(Tier::Llm));
        assert!(!Tier::Knowledge.may_link_to(Tier::Vault));
    }

    #[test]
    fn flags_knowledge_to_vault_link() {
        let tmp = tempfile::tempdir().unwrap();
        let org = tmp.path();
        std::fs::create_dir_all(org.join("vault/Journal")).unwrap();
        std::fs::create_dir_all(org.join("wiki/Knowledge")).unwrap();
        std::fs::write(org.join("vault/Journal/2026-05-22.md"), "today notes").unwrap();
        std::fs::write(
            org.join("wiki/Knowledge/Concepts.md"),
            "see [[2026-05-22]] for context",
        )
        .unwrap();

        let violations = lint_org_tree(org);
        assert_eq!(violations.len(), 1);
        let v = &violations[0];
        assert_eq!(v.source_tier, Tier::Knowledge);
        assert_eq!(v.resolved_tier, Tier::Vault);
        assert_eq!(v.target_link, "2026-05-22");
    }

    #[test]
    fn allows_llm_to_knowledge_link() {
        let tmp = tempfile::tempdir().unwrap();
        let org = tmp.path();
        std::fs::create_dir_all(org.join("wiki/Knowledge")).unwrap();
        std::fs::create_dir_all(org.join("wiki/LLM/Memories")).unwrap();
        std::fs::write(
            org.join("wiki/Knowledge/Loro.md"),
            "CRDT library used by Task",
        )
        .unwrap();
        std::fs::write(
            org.join("wiki/LLM/Memories/2026-05-22.md"),
            "recalling [[Loro]] for the editor work",
        )
        .unwrap();

        assert!(lint_org_tree(org).is_empty());
    }

    #[test]
    fn flags_llm_to_vault_link() {
        let tmp = tempfile::tempdir().unwrap();
        let org = tmp.path();
        std::fs::create_dir_all(org.join("vault/Projects")).unwrap();
        std::fs::create_dir_all(org.join("wiki/LLM/Journals")).unwrap();
        std::fs::write(org.join("vault/Projects/SecretWork.md"), "personal project").unwrap();
        std::fs::write(
            org.join("wiki/LLM/Journals/yesterday.md"),
            "worked on [[SecretWork]]",
        )
        .unwrap();

        let violations = lint_org_tree(org);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].source_tier, Tier::Llm);
        assert_eq!(violations[0].resolved_tier, Tier::Vault);
    }

    #[test]
    fn skips_federated_at_org_links() {
        let tmp = tempfile::tempdir().unwrap();
        let org = tmp.path();
        std::fs::create_dir_all(org.join("wiki/Knowledge")).unwrap();
        std::fs::write(
            org.join("wiki/Knowledge/Local.md"),
            "see [[@other/Page]] for the cross-org reference",
        )
        .unwrap();

        assert!(lint_org_tree(org).is_empty());
    }
}
