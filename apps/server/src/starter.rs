//! Notes every vault starts with.
//!
//! One today: **Example Formatting**, a tour of everything a note can do
//! (`starter/example-formatting.md`). It is planted into an org's vault
//! the first time the org boots — a new org, the seeded studio, and
//! vaults that existed before it did (the phone's own node included) —
//! and only once: a marker in the org's data directory, outside the
//! vault so it never syncs as a note, records the planting, so a note
//! the person deleted or renamed stays deleted or renamed.
//!
//! Peers each plant their own copy. The bytes are the same everywhere,
//! and a document seeded from the same file is the same document on
//! every node (`vault_doc::seed_from_file`), so two devices meeting do
//! not see two notes or a conflict.

use std::path::Path;

/// The vault-relative path the note is planted at.
pub(crate) const EXAMPLE_FORMATTING_PATH: &str = "Example Formatting.md";

/// The note itself.
pub(crate) const EXAMPLE_FORMATTING: &str = include_str!("starter/example-formatting.md");

/// The marker, in the org's data directory.
const MARKER: &str = ".planted-example-formatting";

/// Plant the starter notes into `vault_root` unless `org_dir` says they
/// were planted before. A note already at the path (the person's own) is
/// left alone. Returns whether it planted.
pub(crate) fn plant(vault_root: &Path, org_dir: &Path) -> std::io::Result<bool> {
    let marker = org_dir.join(MARKER);
    if marker.exists() {
        return Ok(false);
    }
    let note = vault_root.join(EXAMPLE_FORMATTING_PATH);
    let planted = if note.exists() {
        false
    } else {
        std::fs::create_dir_all(vault_root)?;
        std::fs::write(&note, EXAMPLE_FORMATTING)?;
        true
    };
    std::fs::create_dir_all(org_dir)?;
    std::fs::write(&marker, b"")?;
    Ok(planted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_once_and_a_deletion_sticks() {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, org) = (tmp.path().join("vault"), tmp.path().join("org"));
        assert!(plant(&vault, &org).unwrap());
        let note = vault.join(EXAMPLE_FORMATTING_PATH);
        assert_eq!(std::fs::read_to_string(&note).unwrap(), EXAMPLE_FORMATTING);

        std::fs::remove_file(&note).unwrap();
        assert!(!plant(&vault, &org).unwrap(), "planted once only");
        assert!(!note.exists(), "a deleted starter note stays deleted");
    }

    #[test]
    fn a_note_of_the_same_name_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, org) = (tmp.path().join("vault"), tmp.path().join("org"));
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join(EXAMPLE_FORMATTING_PATH), "mine").unwrap();
        assert!(!plant(&vault, &org).unwrap());
        assert_eq!(
            std::fs::read_to_string(vault.join(EXAMPLE_FORMATTING_PATH)).unwrap(),
            "mine"
        );
    }

    #[test]
    fn the_note_shows_each_kind_of_formatting() {
        for construct in [
            "**Bold**",
            "- [ ] ",
            "> [!note]",
            "| Song",
            "$$",
            "[[",
            "```",
            "---\n\nThe",
        ] {
            assert!(
                EXAMPLE_FORMATTING.contains(construct),
                "{construct} missing"
            );
        }
    }
}
