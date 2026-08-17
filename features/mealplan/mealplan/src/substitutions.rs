//! Substitution registry — the third (and most composable)
//! sub layer. Lives as standalone markdown pages
//! (`type: substitution`) under `<vault>/substitutions/`
//! so the substitution graph evolves independently of any
//! single recipe or pantry item.
//!
//! Three-layer precedence (matches
//! mealplan/grocy parity, phase 8):
//!
//! 1. **Recipe-ingredient subs** — author intent ("for this
//!    dish, X behaves like Y"). Most specific; checked first.
//! 2. **Pantry-item subs** — global pantry knowledge,
//!    bidirectional ("I can use coconut oil instead of
//!    butter"). Edit once, all recipes benefit.
//! 3. **Registry rules** — the composable graph. Best for
//!    cross-cutting goals ("any high-protein flour swap")
//!    and for (future) fitness integration.
//!
//! Fulfillment consults all three layers in that order and
//! surfaces every viable suggestion on the [`crate::Shortage`]
//! row — caller picks. We never auto-apply.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use facet::Facet;
use pantry::SubReason;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use vault::{Vault, VaultPage};

/// `Vec<SubReason>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct SubReasons(pub Vec<SubReason>);

impl SubReasons {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<SubReason>> for SubReasons {
    fn from(v: Vec<SubReason>) -> Self {
        Self(v)
    }
}

impl FromIterator<SubReason> for SubReasons {
    fn from_iter<I: IntoIterator<Item = SubReason>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for SubReasons {
    type Target = Vec<SubReason>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "substitution_rules", repo)]
pub struct SubstitutionRule {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Display name — `"Butter → Olive Oil"`. Free-form;
    /// purely for human-readable listings.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Source pantry item — the ingredient you'd normally
    /// use.
    #[serde(rename = "fromItemId")]
    #[architect(filterable)]
    pub from_item_id: Uuid,

    /// Substitute pantry item — what you'd use instead.
    #[serde(rename = "toItemId")]
    #[architect(filterable)]
    pub to_item_id: Uuid,

    /// `units_of_substitute / unit_of_original`. Same as
    /// the ratio fields on [`cookbook::Substitution`] and
    /// [`pantry::Substitution`].
    #[serde(default = "default_ratio")]
    pub ratio: f64,

    /// Why this swap is worth offering. Drives goal-filter.
    #[serde(default)]
    #[architect(json)]
    pub reasons: SubReasons,

    /// Tags — `"baking"`, `"dressing-only"`, `"keto"`.
    /// Drives queryable filtering.
    #[serde(skip_serializing_if = "crate::model::StringList::is_empty", default)]
    #[architect(json)]
    pub tags: crate::model::StringList,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    #[serde(skip)]
    pub details: String,
}

fn default_ratio() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum SubstitutionError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait SubstitutionService {
    fn list(&self) -> Result<Vec<SubstitutionRule>, SubstitutionError>;

    fn get(&self, id: &str) -> Result<SubstitutionRule, SubstitutionError>;

    fn create(&self, rule: SubstitutionRule) -> Result<SubstitutionRule, SubstitutionError>;

    fn update(&self, rule: SubstitutionRule) -> Result<SubstitutionRule, SubstitutionError>;

    fn delete(&self, id: &str) -> Result<(), SubstitutionError>;

    /// All registry rules that source `from_item_id`.
    /// Single-direction lookup — symmetry isn't assumed
    /// because "butter → olive oil" doesn't always imply
    /// "olive oil → butter".
    fn for_item(&self, from_item_id: &str) -> Result<Vec<SubstitutionRule>, SubstitutionError>;
}

// ── Parse / write ────────────────────────────────────────────

fn split_frontmatter(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

#[must_use]
pub fn looks_like_substitution_rule(page: &VaultPage) -> bool {
    let Some((fm, _)) = split_frontmatter(&page.raw) else {
        return false;
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(fm) else {
        return false;
    };
    map.get("type").and_then(|v| v.as_str()) == Some("substitution")
}

fn parse_page(page: &VaultPage) -> Option<SubstitutionRule> {
    let (fm, body) = split_frontmatter(&page.raw)?;
    let map: serde_yaml::Mapping = serde_yaml::from_str(fm).ok()?;
    let take_str = |k: &str| {
        map.get(k).and_then(|v| match v {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let id = take_str("id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));
    let name = take_str("name").unwrap_or_else(|| page.basename.clone());
    let from_item_id = take_str("fromItemId").and_then(|s| Uuid::parse_str(&s).ok())?;
    let to_item_id = take_str("toItemId").and_then(|s| Uuid::parse_str(&s).ok())?;
    let ratio = map
        .get("ratio")
        .and_then(serde_yaml::Value::as_f64)
        .unwrap_or(1.0);
    let reasons = map
        .get("reasons")
        .and_then(|v| v.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|v| v.as_str())
                .filter_map(SubReason::from_str)
                .collect()
        })
        .unwrap_or_default();
    let tags = map
        .get("tags")
        .and_then(|v| v.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|v| v.as_str())
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let note = take_str("note");
    let date_created = take_str("dateCreated").and_then(|s| s.parse().ok());
    let date_modified = take_str("dateModified").and_then(|s| s.parse().ok());

    Some(SubstitutionRule {
        path: page.rel_path.clone(),
        id,
        name,
        from_item_id,
        to_item_id,
        ratio,
        reasons,
        tags,
        note,
        date_created,
        date_modified,
        details: body.to_string(),
    })
}

fn serialize(rule: &SubstitutionRule) -> Result<String, SubstitutionError> {
    let mut wrapper = serde_yaml::Mapping::new();
    wrapper.insert("type".into(), "substitution".into());
    let body = serde_yaml::to_value(rule).map_err(|e| SubstitutionError::Io(e.to_string()))?;
    if let serde_yaml::Value::Mapping(m) = body {
        for (k, v) in m {
            wrapper.insert(k, v);
        }
    }
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(wrapper))
        .map_err(|e| SubstitutionError::Io(e.to_string()))?;
    let details = if rule.details.is_empty() {
        String::new()
    } else if rule.details.starts_with('\n') {
        rule.details.clone()
    } else {
        format!("\n{}", rule.details)
    };
    Ok(format!("---\n{yaml}---\n{details}"))
}

fn default_path(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                slug.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("substitution");
    }
    format!("substitutions/{slug}.md")
}

// ── Store ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Vault>>,
}

impl Store {
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        Self {
            inner: Arc::new(Mutex::new(vault)),
        }
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self { inner }
    }

    #[must_use]
    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.clone()
    }
}

fn map_io(e: impl std::fmt::Display) -> SubstitutionError {
    SubstitutionError::Io(e.to_string())
}

fn find_idx(vault: &Vault, id: Uuid) -> Option<usize> {
    vault
        .pages
        .iter()
        .position(|p| looks_like_substitution_rule(p) && parse_page(p).is_some_and(|r| r.id == id))
}

impl architect::HasDispatcher for Store {
    type Dispatcher = architect::dispatch::TokioBlockingDispatcher;
    fn dispatcher(&self) -> Self::Dispatcher {
        architect::dispatch::TokioBlockingDispatcher
    }
}

impl SubstitutionService for Store {
    fn list(&self) -> Result<Vec<SubstitutionRule>, SubstitutionError> {
        let guard = self.inner.lock().expect("substitutions store poisoned");
        Ok(guard
            .pages
            .iter()
            .filter(|p| looks_like_substitution_rule(p))
            .filter_map(parse_page)
            .collect())
    }

    fn get(&self, id: &str) -> Result<SubstitutionRule, SubstitutionError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| SubstitutionError::BadRequest(format!("id: {e}")))?;
        let guard = self.inner.lock().expect("substitutions store poisoned");
        for p in guard
            .pages
            .iter()
            .filter(|p| looks_like_substitution_rule(p))
        {
            if let Some(r) = parse_page(p) {
                if r.id == uuid {
                    return Ok(r);
                }
            }
        }
        Err(SubstitutionError::NotFound(id.to_string()))
    }

    fn create(&self, mut rule: SubstitutionRule) -> Result<SubstitutionRule, SubstitutionError> {
        if rule.id.is_nil() {
            rule.id = Uuid::new_v4();
        }
        if rule.path.is_empty() {
            rule.path = default_path(&rule.name);
        }
        let now = Utc::now();
        rule.date_created.get_or_insert(now);
        rule.date_modified = Some(now);
        let body = serialize(&rule)?;
        let mut guard = self.inner.lock().expect("substitutions store poisoned");
        if guard.pages.iter().any(|p| p.rel_path == rule.path) {
            return Err(SubstitutionError::AlreadyExists(rule.path));
        }
        vault::create_page(&mut guard, &rule.path, body).map_err(map_io)?;
        Ok(rule)
    }

    fn update(&self, mut rule: SubstitutionRule) -> Result<SubstitutionRule, SubstitutionError> {
        let mut guard = self.inner.lock().expect("substitutions store poisoned");
        let idx = find_idx(&guard, rule.id)
            .ok_or_else(|| SubstitutionError::NotFound(rule.id.to_string()))?;
        rule.path = guard.pages[idx].rel_path.clone();
        rule.date_modified = Some(Utc::now());
        let body = serialize(&rule)?;
        guard.pages[idx].raw = body;
        let path = rule.path.clone();
        vault::save_page(&mut guard, &path).map_err(map_io)?;
        Ok(rule)
    }

    fn delete(&self, id: &str) -> Result<(), SubstitutionError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| SubstitutionError::BadRequest(format!("id: {e}")))?;
        let mut guard = self.inner.lock().expect("substitutions store poisoned");
        let idx =
            find_idx(&guard, uuid).ok_or_else(|| SubstitutionError::NotFound(id.to_string()))?;
        let path = guard.pages[idx].rel_path.clone();
        vault::delete_page(&mut guard, &path).map_err(map_io)?;
        Ok(())
    }

    fn for_item(&self, from_item_id: &str) -> Result<Vec<SubstitutionRule>, SubstitutionError> {
        let uuid = Uuid::parse_str(from_item_id)
            .map_err(|e| SubstitutionError::BadRequest(format!("from_item_id: {e}")))?;
        Ok(self
            .list()?
            .into_iter()
            .filter(|r| r.from_item_id == uuid)
            .collect())
    }
}
