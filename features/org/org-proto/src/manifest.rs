//! [`OrgManifest`] — the per-org federation identity record.
//!
//! Defined as an `architect::Entity` so the SeaORM `Model` +
//! `OrgManifestRepoStorage<C>` get emitted under the `server`
//! feature: if a server-level `orgs.sqlite` registry ever
//! makes sense, it works against the same schema with zero
//! struct changes.
//!
//! On-disk today: `<data_root>/orgs/<slug>/org.toml`. The
//! file is the *portable, human-readable* representation; the
//! entity is the *schema*. We keep both so:
//!
//! - `rsync codywright/ user@host:/srv/task/` continues to
//!   move the org with a `cat`-able manifest.
//! - Mounting the same schema into a SQLite registry later
//!   needs no struct change, just a different writer.
//!
//! See the federated-platform design for the federation
//! model context.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Deny-list of plugin ids this org has turned off. Empty = everything
/// on — the pre-plugin behaviour, and what every existing `org.toml`
/// (which lacks the field entirely) resolves to. Ids are validated
/// against `task_plugin::CATALOG` at resolution time, not here: an
/// unknown id is warned about and ignored so a newer build's manifest
/// still loads on an older one. `Vec<String>` newtype — JSON column
/// under the SeaORM emission, same pattern as `fitness_proto::Tags`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct DisabledPlugins(pub Vec<String>);

impl DisabledPlugins {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for DisabledPlugins {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for DisabledPlugins {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[architect(table_name = "orgs", repo)]
pub struct OrgManifest {
    /// Stable opaque id, generated at `org init`. Used as
    /// the canonical handle over federation when slugs
    /// collide across hosts.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Stable, URL-safe identifier. Must match the parent
    /// directory name (`<data_root>/orgs/<slug>`). Lowercase
    /// ASCII + digits + `-`. Drift between the slug and the
    /// directory is rejected at load time — see
    /// `org_proto::root::OrgRoot::manifest`.
    #[architect(filterable, sortable, fulltext)]
    pub slug: String,

    /// Human-facing name. Free-form UTF-8.
    #[architect(filterable, sortable, fulltext)]
    pub display_name: String,

    /// `true` for the user's personal/identity-anchor org.
    /// Home orgs are the only ones that carry
    /// `identity.sqlite` (linked-org credentials + aliases).
    #[architect(filterable)]
    pub is_home: bool,

    /// Canonical URL where this org lives in federation.
    /// Empty until the org is first published; peers use this
    /// to validate federated identity claims.
    pub federation_url: String,

    /// Plugins this org has turned off (deny-list; empty = all on).
    /// Core plugins cannot be disabled — resolution ignores them here.
    /// See `task-plugin` for the plugin catalog.
    #[serde(default, skip_serializing_if = "DisabledPlugins::is_empty")]
    #[architect(json)]
    pub disabled_plugins: DisabledPlugins,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Directory name and `slug` field disagree. A federation
    /// peer that knows this org by its directory-derived slug
    /// would silently route to the wrong place if we let it
    /// load — refuse until a human resolves it.
    #[error("slug `{slug}` does not match directory `{dir}`")]
    SlugDirMismatch { slug: String, dir: String },
}

impl OrgManifest {
    /// Build a freshly-minted manifest. Caller is responsible
    /// for choosing a slug that matches the directory it'll
    /// be written into.
    #[must_use]
    pub fn new(slug: impl Into<String>, display_name: impl Into<String>, is_home: bool) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            slug: slug.into(),
            display_name: display_name.into(),
            is_home,
            federation_url: String::new(),
            disabled_plugins: DisabledPlugins::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// TOML round-trip via `serde`. The entity stays the
    /// schema; this is one persistence backend.
    pub fn parse_str(text: &str, source_path: &str) -> Result<Self, ParseError> {
        toml::from_str(text).map_err(|source| ParseError::Toml {
            path: source_path.to_owned(),
            source,
        })
    }

    /// Read + parse `<dir>/org.toml` and assert the `slug`
    /// field matches the directory's basename.
    pub fn load_from_dir(dir: &std::path::Path) -> Result<Self, ParseError> {
        let path = dir.join("org.toml");
        let path_str = path.display().to_string();
        let text = std::fs::read_to_string(&path).map_err(|source| ParseError::Read {
            path: path_str.clone(),
            source,
        })?;
        let manifest = Self::parse_str(&text, &path_str)?;
        let dir_name = dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if manifest.slug != dir_name {
            return Err(ParseError::SlugDirMismatch {
                slug: manifest.slug.clone(),
                dir: dir_name.to_owned(),
            });
        }
        Ok(manifest)
    }

    /// Serialize + write to `<dir>/org.toml`. Creates `dir`
    /// if missing.
    pub fn write_to_dir(&self, dir: &std::path::Path) -> Result<(), ParseError> {
        std::fs::create_dir_all(dir).map_err(|source| ParseError::Write {
            path: dir.display().to_string(),
            source,
        })?;
        let path = dir.join("org.toml");
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).map_err(|source| ParseError::Write {
            path: path.display().to_string(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An org.toml written before the field existed must parse, and an
    /// empty deny-list must not change what a fresh manifest writes —
    /// existing orgs stay byte-stable.
    #[test]
    fn plugins_field_is_backward_compatible() {
        let pre_plugin = r#"
id = "3fa5a6a2-3436-4d34-8f04-1f4a10bb9a1e"
slug = "demo"
display_name = "Demo"
is_home = false
federation_url = ""
created_at = "2026-01-01T00:00:00Z"
updated_at = "2026-01-01T00:00:00Z"
"#;
        let m = OrgManifest::parse_str(pre_plugin, "org.toml").expect("old manifest parses");
        assert!(
            m.disabled_plugins.is_empty(),
            "absent field = all plugins on"
        );

        let out = toml::to_string(&m).expect("serialize");
        assert!(
            !out.contains("disabled_plugins"),
            "empty deny-list must not be written:\n{out}"
        );

        let mut off = m.clone();
        off.disabled_plugins = vec!["email".to_string()].into();
        let out = toml::to_string(&off).expect("serialize");
        assert!(
            out.contains("disabled_plugins"),
            "non-empty list is written:\n{out}"
        );
        let back = OrgManifest::parse_str(&out, "org.toml").expect("round-trip");
        assert_eq!(back.disabled_plugins, off.disabled_plugins);
    }

    #[test]
    fn roundtrip_via_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("codywright");
        let manifest = OrgManifest::new("codywright", "Cody Wright (personal)", true);
        manifest.write_to_dir(&dir).unwrap();
        let loaded = OrgManifest::load_from_dir(&dir).unwrap();
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn slug_dir_mismatch_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("on-disk-name");
        let mut manifest = OrgManifest::new("on-disk-name", "x", false);
        manifest.write_to_dir(&dir).unwrap();
        manifest.slug = "drifted".into();
        manifest.write_to_dir(&dir).unwrap();
        let err = OrgManifest::load_from_dir(&dir).unwrap_err();
        assert!(matches!(err, ParseError::SlugDirMismatch { .. }));
    }
}
