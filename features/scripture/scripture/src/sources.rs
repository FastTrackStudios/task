//! Where a public-domain edition comes from, and the rule that keeps
//! the licensed ones out.
//!
//! `features/wiki/spec/wiki.md` says a Resource declares its rights and
//! that content which may not be redistributed is never persisted
//! (`wiki.resource.rights`). Scripture is where that first bites:
//! `scripture_proto::Availability` already splits the editions into
//! `Bundled` (public domain or openly licensed) and `Api` (fetched per
//! passage under the reader's own key, never stored).
//!
//! So this module can only name sources for `Bundled` editions, and
//! [`source_for`] refuses an `Api` one rather than returning a URL that
//! would be illegal to follow. The refusal is structural: there is no
//! row here to mistype.

use scripture_proto::{Availability, Translation};

/// A downloadable public-domain edition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// Translation id, matching [`Translation::id`].
    pub id: &'static str,
    /// USFM archive to fetch. eBible.org publishes one zip per edition.
    pub url: &'static str,
}

/// Every edition that may be pulled in whole.
///
/// eBible.org's naming is per-edition rather than systematic
/// (`eng-web_usfm.zip` but `engbsb_usfm.zip`), so these are written
/// out rather than derived — each was checked to resolve.
pub const SOURCES: &[Source] = &[
    Source {
        id: "WEB",
        url: "https://ebible.org/Scriptures/eng-web_usfm.zip",
    },
    Source {
        id: "BSB",
        url: "https://ebible.org/Scriptures/engbsb_usfm.zip",
    },
    Source {
        id: "KJV",
        url: "https://ebible.org/Scriptures/eng-kjv_usfm.zip",
    },
];

/// Why an edition cannot be pulled in.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("`{0}` is not an edition this build knows about")]
    Unknown(String),
    #[error(
        "`{id}` is licensed ({license}) and is read per passage through its API, \
         never downloaded — there is no corpus to install"
    )]
    Licensed { id: String, license: &'static str },
    #[error("`{0}` is public domain but no download source is recorded for it")]
    NoSource(String),
}

/// The source for an edition, or the reason there isn't one.
///
/// t[impl wiki.resource.rights] — the rights posture decides whether a
/// corpus can be held at all, and it is read off the edition rather
/// than from the caller's intent. Asking for a licensed edition is an
/// error with its licence in the message, not a silent no-op that
/// leaves someone wondering where their Bible went.
pub fn source_for(id: &str) -> Result<Source, SourceError> {
    let Some(tx) = Translation::lookup(id) else {
        return Err(SourceError::Unknown(id.to_owned()));
    };
    if tx.availability == Availability::Api {
        return Err(SourceError::Licensed {
            id: tx.id.to_owned(),
            license: tx.license,
        });
    }
    SOURCES
        .iter()
        .copied()
        .find(|s| s.id.eq_ignore_ascii_case(tx.id))
        .ok_or_else(|| SourceError::NoSource(tx.id.to_owned()))
}

/// Every edition that can be pulled in, for a caller offering choices.
pub fn installable() -> impl Iterator<Item = Source> {
    SOURCES.iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t[verify wiki.resource.rights] — the licensed editions are the
    /// ones a mistake would be expensive on, so the refusal is a test
    /// rather than a convention.
    #[test]
    fn licensed_editions_have_no_download() {
        for id in ["NIV", "ESV"] {
            let err = source_for(id).expect_err("licensed edition must refuse");
            assert!(
                matches!(err, SourceError::Licensed { .. }),
                "{id}: wrong refusal: {err}"
            );
        }
    }

    #[test]
    fn every_source_is_a_public_domain_edition() {
        for s in SOURCES {
            let tx = Translation::lookup(s.id)
                .unwrap_or_else(|| panic!("{}: source for an unknown edition", s.id));
            assert_eq!(
                tx.availability,
                Availability::Bundled,
                "{}: has a download but is not redistributable",
                s.id
            );
            assert!(s.url.starts_with("https://"), "{}: insecure source", s.id);
        }
    }

    #[test]
    fn public_domain_editions_resolve() {
        assert_eq!(source_for("WEB").unwrap().id, "WEB");
        // Case-insensitive, because a person types `web`.
        assert_eq!(source_for("web").unwrap().id, "WEB");
        assert!(matches!(source_for("nope"), Err(SourceError::Unknown(_))));
    }
}
