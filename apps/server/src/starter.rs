//! Notes every vault starts with.
//!
//! Two: **Home** (`starter/home.md`), the note the vault opens on — the
//! front page the person makes their own, as in Obsidian — and **Example
//! Formatting** (`starter/example-formatting.md`), a tour of everything a
//! note can do. Each is planted into an org's vault the first time the
//! org boots with it — a new org, the seeded studio, and vaults that
//! existed before it did (the phone's own node included) — and only
//! once: a marker per note in the org's data directory, outside the
//! vault so it never syncs as a note, records the planting, so a note
//! the person deleted or renamed stays deleted or renamed.
//!
//! Peers each plant their own copy. The bytes are the same everywhere,
//! and a document seeded from the same file is the same document on
//! every node (`vault_doc::seed_from_file`), so two devices meeting do
//! not see two notes or a conflict.

use std::path::Path;

/// The vault-relative path of the home note — what the vault opens on.
pub const HOME_PATH: &str = "Home.md";

/// The home note as planted.
pub(crate) const HOME: &str = include_str!("starter/home.md");

/// The vault-relative path the formatting tour is planted at.
pub(crate) const EXAMPLE_FORMATTING_PATH: &str = "Example Formatting.md";

/// The formatting tour itself.
pub(crate) const EXAMPLE_FORMATTING: &str = include_str!("starter/example-formatting.md");

/// Every starter note: its path, its text, and its marker in the org's
/// data directory. A new note gets a new marker, so vaults planted
/// before it existed still receive it, once.
const STARTERS: &[(&str, &str, &str)] = &[
    (HOME_PATH, HOME, ".planted-home"),
    (
        EXAMPLE_FORMATTING_PATH,
        EXAMPLE_FORMATTING,
        ".planted-example-formatting",
    ),
];

/// Plant the starter notes into `vault_root`, each unless `org_dir` says
/// it was planted before. A note already at the path (the person's own)
/// is left alone. Returns how many it wrote.
pub(crate) fn plant(vault_root: &Path, org_dir: &Path) -> std::io::Result<usize> {
    let mut wrote = 0;
    for (path, text, marker) in STARTERS {
        let marker = org_dir.join(marker);
        if marker.exists() {
            continue;
        }
        let note = vault_root.join(path);
        if !note.exists() {
            std::fs::create_dir_all(vault_root)?;
            std::fs::write(&note, text)?;
            wrote += 1;
        }
        std::fs::create_dir_all(org_dir)?;
        std::fs::write(&marker, b"")?;
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_once_and_a_deletion_sticks() {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, org) = (tmp.path().join("vault"), tmp.path().join("org"));
        assert_eq!(plant(&vault, &org).unwrap(), 2, "home and the tour");
        assert_eq!(
            std::fs::read_to_string(vault.join(HOME_PATH)).unwrap(),
            HOME
        );
        let note = vault.join(EXAMPLE_FORMATTING_PATH);
        assert_eq!(std::fs::read_to_string(&note).unwrap(), EXAMPLE_FORMATTING);

        std::fs::remove_file(&note).unwrap();
        assert_eq!(plant(&vault, &org).unwrap(), 0, "planted once only");
        assert!(!note.exists(), "a deleted starter note stays deleted");
    }

    #[test]
    fn a_vault_planted_before_home_existed_gets_home_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, org) = (tmp.path().join("vault"), tmp.path().join("org"));
        // The tour's marker alone: what an older build left behind.
        std::fs::create_dir_all(&org).unwrap();
        std::fs::write(org.join(".planted-example-formatting"), b"").unwrap();
        assert_eq!(plant(&vault, &org).unwrap(), 1);
        assert!(vault.join(HOME_PATH).exists());
        assert!(
            !vault.join(EXAMPLE_FORMATTING_PATH).exists(),
            "the tour was planted (and deleted) before; it stays gone"
        );
    }

    #[test]
    fn a_note_of_the_same_name_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, org) = (tmp.path().join("vault"), tmp.path().join("org"));
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join(EXAMPLE_FORMATTING_PATH), "mine").unwrap();
        assert_eq!(plant(&vault, &org).unwrap(), 1, "only Home is new");
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
