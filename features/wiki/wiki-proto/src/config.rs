//! What a wiki declares about itself, beyond its pages.
//!
//! A wiki is a vault that can be published (`wiki.boundary.role`), and
//! publishing raises questions a vault never has to answer: who may
//! find it, who may change it, who may *ask* to change it, and whether
//! a git repository rather than this server is the authority for its
//! contents. Those answers are the wiki's **config**, one JSON document
//! at `<wiki>/_state/wiki.json` — beside the other agent bookkeeping
//! and outside the markdown a person opens (`wiki.local.mount`).
//!
//! The slug is recorded here too, and that is not redundant with the
//! directory name: `wiki.many.identity` says a slug is never reassigned
//! to a different wiki, and the record of *which* wiki a directory is
//! has to live inside it, so a directory copied or restored under a
//! different name is detected rather than silently renamed.
//!
//! IO lives in `wiki-live`; this module is the shape and the rules that
//! hold on any platform, the same split every other type here keeps.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Who may find a wiki, and who may subscribe to it
/// (`wiki.access.visibility`).
///
/// The distinction between `Unlisted` and `Private` is a refusal, not
/// an absence — an unlisted wiki is simply not advertised, a private one
/// turns an outsider away and says so. Conflating them is the mistake
/// the enum exists to make impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Facet)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Visibility {
    /// Listed in discovery; anyone may subscribe.
    Public,
    /// Listed to nobody; anyone holding the reference may subscribe.
    Unlisted,
    /// Not listed, and a subscription from outside the owning org is
    /// refused. The default: promotion is what makes private writing
    /// public, and it must be a choice (`wiki.promote.vault`).
    #[default]
    Private,
}

impl Visibility {
    /// Whether this wiki appears in discovery.
    #[must_use]
    pub const fn is_listed(self) -> bool {
        matches!(self, Self::Public)
    }

    /// Whether an org other than the owner may subscribe.
    ///
    /// Unlisted passes — the reference is the credential — and only
    /// private refuses. This is the one place the two differ for a
    /// subscriber, and it is a refusal rather than a silent miss.
    #[must_use]
    pub const fn admits_outsiders(self) -> bool {
        !matches!(self, Self::Private)
    }

    /// The word a person reads and a CLI accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Private => "private",
        }
    }

    /// Parse the word back. Case-insensitive, because it is typed.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "public" => Some(Self::Public),
            "unlisted" => Some(Self::Unlisted),
            "private" => Some(Self::Private),
            _ => None,
        }
    }
}

/// Who may open an Edit Request against a wiki (`wiki.edit.gate`).
///
/// Independent of who holds Editor: a wiki can accept proposals from
/// anyone who can read it while only two people may land them, or can
/// close proposals entirely while its Editors go on working. "Closed"
/// is a declared state a client can show, so a contributor is told
/// *before* writing that the wiki will not look at what they send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Facet)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum ProposerGate {
    /// Any account that may read the wiki.
    #[default]
    Readers,
    /// Members of the owning org only. An outside subscriber's push is
    /// held rather than published — the request exists, the wiki has
    /// not vouched for its author, and an Editor can still take it up.
    Members,
    /// No requests accepted. Held requests stay held; new ones are
    /// refused with this state named.
    Closed,
}

impl ProposerGate {
    /// The word a person reads and a CLI accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Readers => "readers",
            Self::Members => "members",
            Self::Closed => "closed",
        }
    }

    /// Parse the word back.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "readers" | "anyone" => Some(Self::Readers),
            "members" => Some(Self::Members),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// A git repository this wiki mirrors (`wiki.source.repo`).
///
/// The repository is the source of truth and the wiki is a mirror of
/// it: `commit` is what the pages currently reflect, and a fetch that
/// fails leaves `last_error` set rather than serving stale content as
/// current (`wiki.source.sync`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct RepoSource {
    /// Clone URL. What `git` is given, verbatim.
    pub url: String,
    /// Branch followed. Empty means the remote's default.
    #[serde(default)]
    pub branch: String,
    /// Path inside the repository that becomes the wiki, `docs/` say.
    /// Empty is the repository root.
    #[serde(default)]
    pub path: String,
    /// The commit the mirror currently reflects. Empty until the first
    /// successful sync.
    #[serde(default)]
    pub commit: String,
    /// When that commit was fetched, RFC 3339. Empty until the first
    /// successful sync.
    #[serde(default)]
    pub fetched_at: String,
    /// What the last fetch said when it failed. Empty when the last
    /// fetch succeeded; a wiki with this set is stale and says so.
    #[serde(default)]
    pub last_error: String,
}

/// One wiki's declaration.
///
/// Everything a wiki says about itself that is not a page. Written by
/// `create_wiki` and by the calls that change one field each; read by
/// listing, by the Edit lane, and by every subscriber's resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct WikiConfig {
    /// The stable identity (`wiki.many.identity`). Recorded inside the
    /// wiki so the directory name and the wiki's own claim can be
    /// compared.
    pub slug: String,
    /// Display title. Retitling changes this and nothing a reference
    /// carries.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub visibility: Visibility,
    /// Principals holding Editor on this wiki (`wiki.edit.editor`) —
    /// account ids as the gate resolves them. Empty means the wiki
    /// has not adopted the Edit lane and writes are governed by org
    /// role alone, which is what every pre-existing wiki did.
    #[serde(default)]
    pub editors: Vec<String>,
    /// Who may propose (`wiki.edit.gate`).
    #[serde(default)]
    pub proposers: ProposerGate,
    /// Set when a repository is the authority (`wiki.source.*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RepoSource>,
    /// When the wiki was created, RFC 3339. Informational.
    #[serde(default)]
    pub created_at: String,
}

impl WikiConfig {
    /// A config for a wiki that has never declared anything — what an
    /// existing directory with no `_state/wiki.json` is read as.
    #[must_use]
    pub fn implicit(slug: &str) -> Self {
        Self {
            slug: slug.to_owned(),
            title: String::new(),
            visibility: Visibility::default(),
            editors: Vec::new(),
            proposers: ProposerGate::default(),
            source: None,
            created_at: String::new(),
        }
    }

    /// Whether this principal may accept changes into the wiki.
    #[must_use]
    pub fn is_editor(&self, principal: &str) -> bool {
        self.editors.iter().any(|e| e == principal)
    }

    /// Whether the Edit lane governs writes here.
    ///
    /// A wiki with no Editors declared is one where org role decides,
    /// as it always did. Declaring the first Editor is what turns the
    /// lane on — from then, org membership alone confers nothing
    /// (`wiki.edit.editor`).
    #[must_use]
    pub fn has_edit_lane(&self) -> bool {
        !self.editors.is_empty()
    }

    /// Whether a repository, rather than this server, is the authority.
    #[must_use]
    pub fn is_repo_sourced(&self) -> bool {
        self.source.is_some()
    }
}

/// A wiki's slug from a display name — lowercase, non-alphanumerics
/// collapsed to single hyphens, no leading or trailing hyphen.
///
/// The same rule `org_proto::wiki_slug` applies to the example tree, so
/// the directory under `<org>/wikis/` and the middle of every reference
/// (`acme.test/music-theory::Ionian`) agree. Duplicated rather than
/// shared because `org-proto` cannot depend on this crate and this
/// crate must stay wasm-clean; `org-proto`'s test pins the two to the
/// same answers.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// What `create_wiki` is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NewWiki {
    /// Display title. The slug is derived from it unless `slug` says
    /// otherwise.
    pub title: String,
    /// Explicit slug. Empty derives one from the title.
    #[serde(default)]
    pub slug: String,
    /// One paragraph on what the wiki is for; becomes `purpose.md`.
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub visibility: Visibility,
    /// Mirror a repository instead of starting empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RepoSource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_words_round_trip() {
        for v in [
            Visibility::Public,
            Visibility::Unlisted,
            Visibility::Private,
        ] {
            assert_eq!(Visibility::parse(v.as_str()), Some(v));
        }
        assert_eq!(Visibility::parse("PUBLIC"), Some(Visibility::Public));
        assert_eq!(Visibility::parse("secret"), None);
    }

    /// t[verify wiki.access.visibility] — unlisted and private differ
    /// for an outsider exactly once, and the difference is a refusal.
    #[test]
    fn only_private_refuses_outsiders() {
        assert!(Visibility::Public.admits_outsiders());
        assert!(Visibility::Unlisted.admits_outsiders());
        assert!(!Visibility::Private.admits_outsiders());
        assert!(Visibility::Public.is_listed());
        assert!(!Visibility::Unlisted.is_listed());
        assert!(!Visibility::Private.is_listed());
    }

    #[test]
    fn slugs_match_the_example_trees_rule() {
        assert_eq!(slugify("Music Theory"), "music-theory");
        assert_eq!(slugify("  Bible Study! "), "bible-study");
        assert_eq!(slugify("Cooking"), "cooking");
        assert_eq!(slugify("--"), "");
    }

    #[test]
    fn a_config_serialises_without_an_absent_source() {
        let c = WikiConfig::implicit("theory");
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("source"), "{json}");
        let back: WikiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn a_missing_field_reads_as_its_default() {
        let back: WikiConfig = serde_json::from_str(r#"{"slug":"x"}"#).unwrap();
        assert_eq!(back.visibility, Visibility::Private);
        assert_eq!(back.proposers, ProposerGate::Readers);
        assert!(!back.has_edit_lane());
    }
}
