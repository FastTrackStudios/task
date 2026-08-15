//! `vault_proto::VaultPage` → `Goal`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `goal::parse::*` paths working.
//!
//! Discriminator: `type: goal` in the frontmatter (or `goal` in
//! `tags:`). Missing optional fields fall back to defaults; missing
//! `id` is synthesized from the path so legacy pages still load —
//! callers should `write_goal` to persist.

pub use vault_entity::ParseError;

use vault_entity::VaultEntity;
use vault_proto::VaultPage;

use crate::entity::Goals;
use crate::model::Goal;

/// True when `page` carries `type: goal` (or the tag).
#[must_use]
pub fn looks_like_goal(page: &VaultPage) -> bool {
    vault_entity::frontmatter::has_type(&page.raw, Goals::TYPE)
}

/// Parse a goal page.
pub fn parse_page(page: &VaultPage) -> Result<Goal, ParseError> {
    parse_goal(&page.rel_path, &page.basename, &page.raw)
}

/// Parse goal frontmatter + body. Lower-level surface for
/// callers that don't have a `VaultPage` handy (e.g. CLI
/// importers, migration scripts).
pub fn parse_goal(rel_path: &str, basename: &str, raw: &str) -> Result<Goal, ParseError> {
    crate::entity::from_parts(rel_path, basename, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tags;

    #[test]
    fn round_trip_minimal_goal() {
        let raw = "---\ntype: goal\nid: 550e8400-e29b-41d4-a716-446655440000\ntitle: Buy a House\nkind: lifetime\nstatus: aspiration\ntags:\n- housing\n- long-term\n---\nVision: own a place by 35.\n";
        let g = parse_goal("Goals/buy-a-house.md", "buy-a-house", raw).unwrap();
        assert_eq!(g.title, "Buy a House");
        assert_eq!(g.kind, "lifetime");
        assert_eq!(g.status, "aspiration");
        assert_eq!(g.tags, Tags(vec!["housing".into(), "long-term".into()]));
        assert!(g.details.contains("Vision"));
    }

    #[test]
    fn accepts_snake_case_parent_id() {
        let raw = "---\ntype: goal\nid: 550e8400-e29b-41d4-a716-446655440000\nparent_id: 660e8400-e29b-41d4-a716-446655440000\n---\nbody\n";
        let g = parse_goal("Goals/x.md", "x", raw).unwrap();
        assert!(g.parent_id.is_some());
    }
}
