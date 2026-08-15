//! `VaultPage` — wasm-clean, transport-friendly view of one
//! markdown file in a vault.
//!
//! Parsers (`project::parse_page`, `goal::parse_page`, `task::parse_page`,
//! `cookbook::parse_cook`, …) consume this shape so they work
//! unchanged in either context:
//!
//! - **Server-side**: `vault::Vault::open(path)` walks the
//!   filesystem and converts each `vault_live::VaultPage` →
//!   `vault_proto::VaultPage` for the parser.
//! - **Wasm-side**: a `VaultReadService` (future) streams the
//!   same struct from `/org/<slug>/vox` so a client UI can
//!   parse-as-if-local without holding any filesystem.
//!
//! Pure data — no `SystemTime`, no `PathBuf`. `mtime_ms`
//! travels as a Unix-ms integer because wasm targets can't
//! always round-trip `SystemTime`.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// One `.md` page, transport-shaped.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct VaultPage {
    /// Vault-relative path, forward-slash separated
    /// (`Notes/2026/howdy.md`). Stable identity.
    pub rel_path: String,
    /// Filename without extension. Wikilink target.
    pub basename: String,
    /// Parent directory inside the vault; empty at the root.
    pub folder: String,
    /// Raw file bytes at scan time. Parsers split frontmatter
    /// + body out of this.
    pub raw: String,
    /// `mtime` snapshot in Unix milliseconds. Zero means
    /// unknown / not tracked.
    pub mtime_ms: i64,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::VaultPage;
    unsafe impl vox_types::Reborrow for VaultPage {
        type Ref<'a> = VaultPage;
    }
}
