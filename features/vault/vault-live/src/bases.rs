//! Obsidian Bases (.base YAML) parser. Bases are saved queries over
//! the vault, with one or more views (table / board / gallery /
//! calendar / list). See <https://help.obsidian.md/bases/syntax> for
//! the wire syntax. The leaf filter strings are parsed by a tiny
//! expression parser (see [`expr_parser`]) — recognized identifier
//! prefixes:
//!
//! - `file.<name>` → [`Expr::FileProp`] (file.name, file.mtime, …)
//! - `note.<name>` → [`Expr::NoteProp`] (frontmatter access)
//! - `formula.<name>` → [`Expr::FormulaRef`]
//! - bare `<name>` → [`Expr::NoteProp`]
//!
//! Function calls (`recv.fn(arg, …)`) are parsed generically. The
//! evaluator is [`BaseEval`] (below; driven by [`execute_view`] and
//! [`view_table`]): it runs `formulas:`, maps `hasTag` / `hasLink` /
//! `exists` / `hasProperty` / `inFolder` / `contains` / `startsWith` /
//! `endsWith` on top of [`FilterNode::Call`], exposes `file.links`, and
//! follows link values (`note.song.key` reads the linked note's `key`).
//! Unknown functions evaluate to `null` rather than failing the view.

use serde::{Deserialize, Serialize};

// ── AST ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParsedBase {
    pub global_filter: FilterNode,
    pub formulas: Vec<Formula>,
    /// Ordered — controls default column layout.
    pub properties: Vec<PropertyConfig>,
    pub views: Vec<ViewSpec>,
    /// Top-level `folder:` wikilink → parent note basename, so the base
    /// shows up under its folder in the vault tree. Empty = root.
    pub folder: String,
    /// Top-level `tags:` — puts the base in the tags sidebar.
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum FilterNode {
    And {
        args: Vec<FilterNode>,
    },
    Or {
        args: Vec<FilterNode>,
    },
    Not {
        arg: Box<FilterNode>,
    },
    Cmp {
        left: Expr,
        op: CmpOp,
        right: Expr,
    },
    Call {
        receiver: Expr,
        name: String,
        args: Vec<Expr>,
    },
    Truthy {
        expr: Expr,
    },
    /// Empty filter — matches everything.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expr {
    /// `file.name`, `file.mtime`, `file.ctime`, `file.path`,
    /// `file.size`, `file.ext`, `file.folder`.
    FileProp { name: String },
    /// `note.author` or bare `status` — frontmatter access.
    NoteProp { name: String },
    /// `formula.foo` — reference into the `formulas:` block.
    FormulaRef { name: String },
    /// Literal JSON value (string, number, bool, null, list, map).
    Literal { value: serde_json::Value },
    /// `this` — current page in templated bases (Project Hierarchy,
    /// relationships).
    This,
    /// Free function call (`list(x)`, `today()`, `date(due)`) when
    /// `receiver` is `None`, or method call (`x.contains(y)`,
    /// `date(due).format("YYYY-MM")`) when `receiver` is `Some`.
    /// The evaluator treats unknown calls as `null` for now —
    /// keeping this in the AST lets us round-trip the source.
    Call {
        receiver: Option<Box<Expr>>,
        name: String,
        args: Vec<Expr>,
    },
    /// `a + b` / `a - b` / `a * b` / `a / b` / `a % b`. Also covers
    /// string concatenation via `+`.
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `-x` — only unary minus right now. (`!` is a `FilterNode` op,
    /// not an Expr op.)
    Unary { op: UnaryOp, arg: Box<Expr> },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Logical AND inside expressions (`a && b`, `a and b`). Distinct
    /// from `FilterNode::And` which lives at the filter level.
    And,
    /// Logical OR inside expressions.
    Or,
    /// Comparison ops inside expressions — for cases like
    /// `if(a == b, …)` where the comparison sits in expression
    /// position rather than at a filter root.
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    /// Logical negation inside expressions (`!x`, `not x`).
    Not,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Formula {
    pub name: String,
    /// Raw source — evaluated on demand by the evaluator.
    pub expression: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyConfig {
    pub key: String,
    pub display_name: Option<String>,
    /// Date format, number locale, etc.
    pub format: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    pub kind: ViewKind,
    pub name: String,
    /// View-scoped — AND-ed onto `global_filter` at run time.
    pub filter: Option<FilterNode>,
    /// Visible columns / projection.
    pub order: Vec<String>,
    pub sort: Vec<SortKey>,
    pub limit: Option<u32>,
    pub group_by: Option<String>,
    /// Kind-specific (image property, card size, date property,
    /// columns…). Kept opaque so the schema can grow.
    pub extras: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ViewKind {
    Table,
    /// Obsidian's grid-of-cards view (`type: cards`). `Gallery` is kept
    /// as a back-compat alias for the same renderer.
    Cards,
    Board,
    Gallery,
    Calendar,
    List,
    /// Plugin-supplied view type the core engine doesn't know how
    /// to render. We still parse the surrounding metadata and the
    /// UI can fall back to a placeholder card. Matches Obsidian's
    /// behavior — it lists `.base` files with unknown view types
    /// rather than rejecting them.
    Other(String),
}

impl ViewKind {
    /// Canonical lowercase tag (`table`, `board`, …) — what the UI keys
    /// its renderer on.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            ViewKind::Table => "table",
            ViewKind::Cards => "cards",
            ViewKind::Board => "board",
            ViewKind::Gallery => "gallery",
            ViewKind::Calendar => "calendar",
            ViewKind::List => "list",
            ViewKind::Other(s) => s,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SortKey {
    pub property: String,
    pub direction: SortDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(thiserror::Error, Debug)]
pub enum BaseParseError {
    #[error("yaml: {0}")]
    Yaml(String),
    #[error("invalid filter: {0}")]
    Filter(String),
    #[error("invalid view: {0}")]
    View(String),
    #[error("invalid expression: {0}")]
    Expr(String),
}

// ── Top-level parse / serialize ──────────────────────────────────────

/// Parse a .base YAML string into a typed AST.
pub fn parse(yaml: &str) -> Result<ParsedBase, BaseParseError> {
    let root: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| BaseParseError::Yaml(e.to_string()))?;
    let map = match &root {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => {
            return Ok(ParsedBase {
                global_filter: FilterNode::None,
                formulas: Vec::new(),
                properties: Vec::new(),
                views: Vec::new(),
                folder: String::new(),
                tags: Vec::new(),
            });
        }
        _ => return Err(BaseParseError::Yaml("root must be a mapping".into())),
    };

    let global_filter = match map.get(serde_yaml::Value::String("filters".into())) {
        Some(v) => parse_filter_node(v)?,
        None => FilterNode::None,
    };

    let formulas = match map.get(serde_yaml::Value::String("formulas".into())) {
        Some(serde_yaml::Value::Mapping(fm)) => {
            let mut out = Vec::new();
            for (k, v) in fm {
                let name = yaml_str(k)
                    .ok_or_else(|| BaseParseError::Yaml("formula name not string".into()))?
                    .to_string();
                let expression = yaml_str(v)
                    .ok_or_else(|| BaseParseError::Yaml("formula body not string".into()))?
                    .to_string();
                out.push(Formula { name, expression });
            }
            out
        }
        Some(_) => return Err(BaseParseError::Yaml("formulas must be a mapping".into())),
        None => Vec::new(),
    };

    let properties = match map.get(serde_yaml::Value::String("properties".into())) {
        Some(serde_yaml::Value::Mapping(pm)) => {
            let mut out = Vec::new();
            for (k, v) in pm {
                let key = yaml_str(k)
                    .ok_or_else(|| BaseParseError::Yaml("property key not string".into()))?
                    .to_string();
                let (display_name, format) = match v {
                    serde_yaml::Value::Mapping(m) => (
                        m.get(serde_yaml::Value::String("displayName".into()))
                            .and_then(yaml_str)
                            .map(str::to_string),
                        m.get(serde_yaml::Value::String("format".into()))
                            .and_then(yaml_str)
                            .map(str::to_string),
                    ),
                    serde_yaml::Value::Null => (None, None),
                    _ => (None, None),
                };
                out.push(PropertyConfig {
                    key,
                    display_name,
                    format,
                });
            }
            out
        }
        Some(_) => return Err(BaseParseError::Yaml("properties must be a mapping".into())),
        None => Vec::new(),
    };

    let views = match map.get(serde_yaml::Value::String("views".into())) {
        Some(serde_yaml::Value::Sequence(vs)) => {
            let mut out = Vec::with_capacity(vs.len());
            for v in vs {
                out.push(parse_view(v)?);
            }
            out
        }
        Some(_) => return Err(BaseParseError::View("views must be a sequence".into())),
        None => Vec::new(),
    };

    // Top-level `folder:` (wikilink → parent note) and `tags:` — let
    // `.base` files sit in the folder tree + tags sidebar exactly like
    // markdown pages. `folder` gets the same wikilink stripping the page
    // frontmatter path uses in sync.rs; `tags` accepts a sequence of
    // strings or a single scalar.
    let folder = map
        .get(serde_yaml::Value::String("folder".into()))
        .and_then(yaml_str)
        .map(strip_wikilink)
        .unwrap_or_default();

    let tags = match map.get(serde_yaml::Value::String("tags".into())) {
        Some(serde_yaml::Value::Sequence(s)) => s
            .iter()
            .filter_map(yaml_str)
            .map(normalize_tag)
            .filter(|t| !t.is_empty())
            .collect(),
        Some(serde_yaml::Value::String(s)) => {
            let t = normalize_tag(s);
            if t.is_empty() { Vec::new() } else { vec![t] }
        }
        _ => Vec::new(),
    };

    Ok(ParsedBase {
        global_filter,
        formulas,
        properties,
        views,
        folder,
        tags,
    })
}

/// Reduce a `folder` value to a bare parent basename — mirrors
/// `sync::strip_wikilink` so a base's `folder:` resolves in the tree
/// identically to a page's frontmatter `folder`. Handles the Obsidian
/// wikilink form `[[Name|alias]]#heading` as well as a plain string.
fn strip_wikilink(value: &str) -> String {
    let t = value.trim();
    let inner = t
        .strip_prefix("[[")
        .and_then(|x| x.strip_suffix("]]"))
        .unwrap_or(t);
    inner
        .split(['|', '#'])
        .next()
        .unwrap_or(inner)
        .trim()
        .to_string()
}

/// Normalize a single tag — trim whitespace and a leading `#`, mirroring
/// `sync::fm_tags`.
fn normalize_tag(raw: &str) -> String {
    raw.trim().trim_start_matches('#').to_string()
}

/// Serialize back to YAML — used when the user edits a Base in our UI.
pub fn serialize(b: &ParsedBase) -> Result<String, BaseParseError> {
    let mut root = serde_yaml::Mapping::new();
    if !matches!(b.global_filter, FilterNode::None) {
        root.insert(
            serde_yaml::Value::String("filters".into()),
            filter_to_yaml(&b.global_filter)?,
        );
    }
    if !b.formulas.is_empty() {
        let mut fm = serde_yaml::Mapping::new();
        for f in &b.formulas {
            fm.insert(
                serde_yaml::Value::String(f.name.clone()),
                serde_yaml::Value::String(f.expression.clone()),
            );
        }
        root.insert(
            serde_yaml::Value::String("formulas".into()),
            serde_yaml::Value::Mapping(fm),
        );
    }
    if !b.properties.is_empty() {
        let mut pm = serde_yaml::Mapping::new();
        for p in &b.properties {
            let mut entry = serde_yaml::Mapping::new();
            if let Some(d) = &p.display_name {
                entry.insert(
                    serde_yaml::Value::String("displayName".into()),
                    serde_yaml::Value::String(d.clone()),
                );
            }
            if let Some(f) = &p.format {
                entry.insert(
                    serde_yaml::Value::String("format".into()),
                    serde_yaml::Value::String(f.clone()),
                );
            }
            pm.insert(
                serde_yaml::Value::String(p.key.clone()),
                serde_yaml::Value::Mapping(entry),
            );
        }
        root.insert(
            serde_yaml::Value::String("properties".into()),
            serde_yaml::Value::Mapping(pm),
        );
    }
    if !b.views.is_empty() {
        let mut vs = Vec::with_capacity(b.views.len());
        for v in &b.views {
            vs.push(view_to_yaml(v)?);
        }
        root.insert(
            serde_yaml::Value::String("views".into()),
            serde_yaml::Value::Sequence(vs),
        );
    }
    serde_yaml::to_string(&serde_yaml::Value::Mapping(root))
        .map_err(|e| BaseParseError::Yaml(e.to_string()))
}

// ── Helpers ──────────────────────────────────────────────────────────

fn yaml_str(v: &serde_yaml::Value) -> Option<&str> {
    match v {
        serde_yaml::Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn parse_view(v: &serde_yaml::Value) -> Result<ViewSpec, BaseParseError> {
    let m = match v {
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err(BaseParseError::View("view must be a mapping".into())),
    };
    let kind_str = m
        .get(serde_yaml::Value::String("type".into()))
        .and_then(yaml_str)
        .ok_or_else(|| BaseParseError::View("view missing `type`".into()))?;
    let kind = match kind_str {
        "table" => ViewKind::Table,
        "cards" => ViewKind::Cards,
        "board" => ViewKind::Board,
        "gallery" => ViewKind::Gallery,
        "calendar" => ViewKind::Calendar,
        "list" => ViewKind::List,
        other => ViewKind::Other(other.to_string()),
    };
    let name = m
        .get(serde_yaml::Value::String("name".into()))
        .and_then(yaml_str)
        .unwrap_or("")
        .to_string();
    let filter = match m.get(serde_yaml::Value::String("filters".into())) {
        Some(node) => Some(parse_filter_node(node)?),
        None => None,
    };
    let order = match m.get(serde_yaml::Value::String("order".into())) {
        Some(serde_yaml::Value::Sequence(s)) => {
            s.iter().filter_map(yaml_str).map(str::to_string).collect()
        }
        _ => Vec::new(),
    };
    let sort = match m.get(serde_yaml::Value::String("sort".into())) {
        Some(serde_yaml::Value::Sequence(s)) => {
            let mut out = Vec::new();
            for entry in s {
                let em = match entry {
                    serde_yaml::Value::Mapping(m) => m,
                    _ => continue,
                };
                let property = em
                    .get(serde_yaml::Value::String("property".into()))
                    .and_then(yaml_str)
                    .unwrap_or("")
                    .to_string();
                let direction = match em
                    .get(serde_yaml::Value::String("direction".into()))
                    .and_then(yaml_str)
                {
                    Some("DESC" | "desc") => SortDir::Desc,
                    _ => SortDir::Asc,
                };
                out.push(SortKey {
                    property,
                    direction,
                });
            }
            out
        }
        _ => Vec::new(),
    };
    let limit = m
        .get(serde_yaml::Value::String("limit".into()))
        .and_then(serde_yaml::Value::as_u64)
        .map(|n| n as u32);
    // `groupBy` is an object `{ property, direction }` in current
    // Obsidian; older/our files used a bare string. Accept both (we key
    // grouping on the property; the group direction isn't applied yet).
    let group_by = match m.get(serde_yaml::Value::String("groupBy".into())) {
        Some(serde_yaml::Value::Mapping(gm)) => gm
            .get(serde_yaml::Value::String("property".into()))
            .and_then(yaml_str)
            .map(str::to_string),
        Some(other) => yaml_str(other).map(str::to_string),
        None => None,
    };

    // Extras = everything we didn't claim. Convert to JSON via yaml→json.
    let mut extras_map = serde_yaml::Mapping::new();
    for (k, v) in m {
        let key = match yaml_str(k) {
            Some(s) => s,
            None => continue,
        };
        if matches!(
            key,
            "type" | "name" | "filters" | "order" | "sort" | "limit" | "groupBy"
        ) {
            continue;
        }
        extras_map.insert(k.clone(), v.clone());
    }
    let extras = yaml_to_json(&serde_yaml::Value::Mapping(extras_map));

    Ok(ViewSpec {
        kind,
        name,
        filter,
        order,
        sort,
        limit,
        group_by,
        extras,
    })
}

fn view_to_yaml(v: &ViewSpec) -> Result<serde_yaml::Value, BaseParseError> {
    let mut m = serde_yaml::Mapping::new();
    let kind_str: &str = match &v.kind {
        ViewKind::Table => "table",
        ViewKind::Cards => "cards",
        ViewKind::Board => "board",
        ViewKind::Gallery => "gallery",
        ViewKind::Calendar => "calendar",
        ViewKind::List => "list",
        ViewKind::Other(s) => s.as_str(),
    };
    m.insert(
        serde_yaml::Value::String("type".into()),
        serde_yaml::Value::String(kind_str.into()),
    );
    if !v.name.is_empty() {
        m.insert(
            serde_yaml::Value::String("name".into()),
            serde_yaml::Value::String(v.name.clone()),
        );
    }
    if let Some(f) = &v.filter {
        m.insert(
            serde_yaml::Value::String("filters".into()),
            filter_to_yaml(f)?,
        );
    }
    if !v.order.is_empty() {
        m.insert(
            serde_yaml::Value::String("order".into()),
            serde_yaml::Value::Sequence(
                v.order
                    .iter()
                    .map(|s| serde_yaml::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !v.sort.is_empty() {
        let seq = v
            .sort
            .iter()
            .map(|s| {
                let mut em = serde_yaml::Mapping::new();
                em.insert(
                    serde_yaml::Value::String("property".into()),
                    serde_yaml::Value::String(s.property.clone()),
                );
                em.insert(
                    serde_yaml::Value::String("direction".into()),
                    serde_yaml::Value::String(
                        match s.direction {
                            SortDir::Asc => "ASC",
                            SortDir::Desc => "DESC",
                        }
                        .into(),
                    ),
                );
                serde_yaml::Value::Mapping(em)
            })
            .collect();
        m.insert(
            serde_yaml::Value::String("sort".into()),
            serde_yaml::Value::Sequence(seq),
        );
    }
    if let Some(l) = v.limit {
        m.insert(
            serde_yaml::Value::String("limit".into()),
            serde_yaml::Value::Number(l.into()),
        );
    }
    if let Some(g) = &v.group_by {
        m.insert(
            serde_yaml::Value::String("groupBy".into()),
            serde_yaml::Value::String(g.clone()),
        );
    }
    if let serde_yaml::Value::Mapping(extras) = json_to_yaml(&v.extras) {
        for (k, val) in extras {
            m.insert(k, val);
        }
    }
    Ok(serde_yaml::Value::Mapping(m))
}

// ── Filter parsing ───────────────────────────────────────────────────

fn parse_filter_node(v: &serde_yaml::Value) -> Result<FilterNode, BaseParseError> {
    match v {
        serde_yaml::Value::Null => Ok(FilterNode::None),
        serde_yaml::Value::String(s) => parse_filter_string(s),
        serde_yaml::Value::Mapping(m) => {
            // Expect exactly one of: and / or / not — or a wrapped
            // expression like `{ filter: "<expr>" }`.
            if let Some(args) = m.get(serde_yaml::Value::String("and".into())) {
                let args = parse_filter_list(args)?;
                return Ok(FilterNode::And { args });
            }
            if let Some(args) = m.get(serde_yaml::Value::String("or".into())) {
                let args = parse_filter_list(args)?;
                return Ok(FilterNode::Or { args });
            }
            if let Some(arg) = m.get(serde_yaml::Value::String("not".into())) {
                return Ok(FilterNode::Not {
                    arg: Box::new(parse_filter_node(arg)?),
                });
            }
            Err(BaseParseError::Filter(
                "filter mapping must use and/or/not".into(),
            ))
        }
        serde_yaml::Value::Sequence(seq) => {
            // Bare sequence = implicit AND.
            let mut args = Vec::new();
            for v in seq {
                args.push(parse_filter_node(v)?);
            }
            Ok(FilterNode::And { args })
        }
        _ => Err(BaseParseError::Filter("invalid filter shape".into())),
    }
}

fn parse_filter_list(v: &serde_yaml::Value) -> Result<Vec<FilterNode>, BaseParseError> {
    match v {
        serde_yaml::Value::Sequence(seq) => {
            let mut args = Vec::with_capacity(seq.len());
            for v in seq {
                args.push(parse_filter_node(v)?);
            }
            Ok(args)
        }
        _ => Err(BaseParseError::Filter("and/or expects a sequence".into())),
    }
}

fn parse_filter_string(src: &str) -> Result<FilterNode, BaseParseError> {
    let src = src.trim();
    if src.is_empty() {
        return Ok(FilterNode::None);
    }
    expr_parser::parse_filter(src)
}

fn filter_to_yaml(f: &FilterNode) -> Result<serde_yaml::Value, BaseParseError> {
    Ok(match f {
        FilterNode::None => serde_yaml::Value::Null,
        FilterNode::And { args } => {
            let seq: Result<Vec<_>, _> = args.iter().map(filter_to_yaml).collect();
            let mut m = serde_yaml::Mapping::new();
            m.insert(
                serde_yaml::Value::String("and".into()),
                serde_yaml::Value::Sequence(seq?),
            );
            serde_yaml::Value::Mapping(m)
        }
        FilterNode::Or { args } => {
            let seq: Result<Vec<_>, _> = args.iter().map(filter_to_yaml).collect();
            let mut m = serde_yaml::Mapping::new();
            m.insert(
                serde_yaml::Value::String("or".into()),
                serde_yaml::Value::Sequence(seq?),
            );
            serde_yaml::Value::Mapping(m)
        }
        FilterNode::Not { arg } => {
            let mut m = serde_yaml::Mapping::new();
            m.insert(
                serde_yaml::Value::String("not".into()),
                filter_to_yaml(arg)?,
            );
            serde_yaml::Value::Mapping(m)
        }
        FilterNode::Cmp { left, op, right } => serde_yaml::Value::String(format!(
            "{} {} {}",
            expr_to_source(left),
            cmp_to_source(*op),
            expr_to_source(right)
        )),
        FilterNode::Call {
            receiver,
            name,
            args,
        } => {
            let arg_src: Vec<String> = args.iter().map(expr_to_source).collect();
            serde_yaml::Value::String(format!(
                "{}.{}({})",
                expr_to_source(receiver),
                name,
                arg_src.join(", ")
            ))
        }
        FilterNode::Truthy { expr } => serde_yaml::Value::String(expr_to_source(expr)),
    })
}

fn cmp_to_source(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Neq => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::Contains => "contains",
        CmpOp::StartsWith => "startsWith",
        CmpOp::EndsWith => "endsWith",
    }
}

fn expr_to_source(e: &Expr) -> String {
    match e {
        Expr::FileProp { name } => {
            if name.is_empty() {
                "file".into()
            } else {
                format!("file.{name}")
            }
        }
        Expr::NoteProp { name } => {
            if name.is_empty() {
                "note".into()
            } else if name.contains('.') {
                format!("note.{name}")
            } else {
                name.clone()
            }
        }
        Expr::FormulaRef { name } => {
            if name.is_empty() {
                "formula".into()
            } else {
                format!("formula.{name}")
            }
        }
        Expr::Literal { value } => match value {
            serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => "null".into(),
            other => other.to_string(),
        },
        Expr::This => "this".into(),
        Expr::Call {
            receiver,
            name,
            args,
        } => {
            let arg_src = args
                .iter()
                .map(expr_to_source)
                .collect::<Vec<_>>()
                .join(", ");
            match receiver {
                Some(r) => format!("{}.{name}({arg_src})", expr_to_source(r)),
                None => format!("{name}({arg_src})"),
            }
        }
        Expr::Binary { op, left, right } => {
            let op = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Rem => "%",
                BinOp::And => "&&",
                BinOp::Or => "||",
                BinOp::Eq => "==",
                BinOp::Neq => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
            };
            format!("{} {op} {}", expr_to_source(left), expr_to_source(right))
        }
        Expr::Unary { op, arg } => {
            let op = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            format!("{op}{}", expr_to_source(arg))
        }
    }
}

// ── YAML <-> JSON ────────────────────────────────────────────────────

fn yaml_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    match v {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number)
            } else {
                serde_json::Value::Null
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_yaml::Value::Sequence(s) => {
            serde_json::Value::Array(s.iter().map(yaml_to_json).collect())
        }
        serde_yaml::Value::Mapping(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => serde_yaml::to_string(other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_json(&t.value),
    }
}

fn json_to_yaml(v: &serde_json::Value) -> serde_yaml::Value {
    match v {
        serde_json::Value::Null => serde_yaml::Value::Null,
        serde_json::Value::Bool(b) => serde_yaml::Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_yaml::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_yaml::Value::Number(f.into())
            } else {
                serde_yaml::Value::Null
            }
        }
        serde_json::Value::String(s) => serde_yaml::Value::String(s.clone()),
        serde_json::Value::Array(a) => {
            serde_yaml::Value::Sequence(a.iter().map(json_to_yaml).collect())
        }
        serde_json::Value::Object(o) => {
            let mut m = serde_yaml::Mapping::new();
            for (k, v) in o {
                m.insert(serde_yaml::Value::String(k.clone()), json_to_yaml(v));
            }
            serde_yaml::Value::Mapping(m)
        }
    }
}

// ── Expression parser (leaf filter strings) ──────────────────────────

pub mod expr_parser {
    //! Tiny recursive-descent parser for Bases filter expressions.
    //!
    //! Grammar:
    //!   filter   := or
    //!   or       := and ("||" and)*
    //!   and      := not ("&&" not)*
    //!   not      := "!" not | cmp
    //!   cmp      := postfix (("==" | "!=" | "<=" | ">=" | "<" | ">") postfix)?
    //!   postfix  := primary ("." IDENT ("(" args? ")")?)*
    //!   primary  := literal | ident | "(" or ")"
    //!   literal  := STRING | NUMBER | "true" | "false" | "null"
    //!   args     := or ("," or)*

    use super::{BaseParseError, BinOp, CmpOp, Expr, FilterNode, UnaryOp};

    pub fn parse_filter(src: &str) -> Result<FilterNode, BaseParseError> {
        let mut p = Parser::new(src);
        let node = p.parse_or()?;
        p.skip_ws();
        if p.pos < p.src.len() {
            return Err(BaseParseError::Expr(format!(
                "trailing input at byte {}: {:?}",
                p.pos,
                &p.src[p.pos..]
            )));
        }
        Ok(node)
    }

    /// Parse one value expression — a `formulas:` entry such as
    /// `if(fee > 0, fee * 0.1, 0)` or `note.song.key + " major"`.
    pub fn parse_expression(src: &str) -> Result<Expr, BaseParseError> {
        let mut p = Parser::new(src);
        let expr = p.parse_expr()?;
        p.skip_ws();
        if p.pos < p.src.len() {
            return Err(BaseParseError::Expr(format!(
                "trailing input at byte {}: {:?}",
                p.pos,
                &p.src[p.pos..]
            )));
        }
        Ok(expr)
    }

    struct Parser<'a> {
        src: &'a [u8],
        pos: usize,
    }

    impl<'a> Parser<'a> {
        fn new(src: &'a str) -> Self {
            Self {
                src: src.as_bytes(),
                pos: 0,
            }
        }

        fn skip_ws(&mut self) {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
        }

        fn peek(&self) -> Option<u8> {
            self.src.get(self.pos).copied()
        }

        fn eat(&mut self, lit: &[u8]) -> bool {
            self.skip_ws();
            if self.src[self.pos..].starts_with(lit) {
                self.pos += lit.len();
                true
            } else {
                false
            }
        }

        fn parse_or(&mut self) -> Result<FilterNode, BaseParseError> {
            let mut left = self.parse_and()?;
            loop {
                if !self.eat_logical_or() {
                    break;
                }
                let right = self.parse_and()?;
                left = match left {
                    FilterNode::Or { mut args } => {
                        args.push(right);
                        FilterNode::Or { args }
                    }
                    other => FilterNode::Or {
                        args: vec![other, right],
                    },
                };
            }
            Ok(left)
        }

        fn parse_and(&mut self) -> Result<FilterNode, BaseParseError> {
            let mut left = self.parse_not()?;
            loop {
                if !self.eat_logical_and() {
                    break;
                }
                let right = self.parse_not()?;
                left = match left {
                    FilterNode::And { mut args } => {
                        args.push(right);
                        FilterNode::And { args }
                    }
                    other => FilterNode::And {
                        args: vec![other, right],
                    },
                };
            }
            Ok(left)
        }

        fn parse_not(&mut self) -> Result<FilterNode, BaseParseError> {
            self.skip_ws();
            if self.peek() == Some(b'!') && self.src.get(self.pos + 1) != Some(&b'=') {
                self.pos += 1;
                let inner = self.parse_not()?;
                return Ok(FilterNode::Not {
                    arg: Box::new(inner),
                });
            }
            if self.eat_keyword(b"not") {
                let inner = self.parse_not()?;
                return Ok(FilterNode::Not {
                    arg: Box::new(inner),
                });
            }
            self.parse_cmp()
        }

        fn parse_cmp(&mut self) -> Result<FilterNode, BaseParseError> {
            // At filter level, the LHS / RHS of a comparison are
            // arithmetic exprs — logical + cmp ops at this level are
            // handled by the surrounding `parse_or` / `parse_and` /
            // `parse_cmp` themselves, not nested inside the operand.
            let left = self.parse_add()?;
            self.skip_ws();
            let op = if self.eat(b"==") {
                Some(CmpOp::Eq)
            } else if self.eat(b"!=") {
                Some(CmpOp::Neq)
            } else if self.eat(b"<=") {
                Some(CmpOp::Le)
            } else if self.eat(b">=") {
                Some(CmpOp::Ge)
            } else if self.eat(b"<") {
                Some(CmpOp::Lt)
            } else if self.eat(b">") {
                Some(CmpOp::Gt)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_add()?;
                    Ok(FilterNode::Cmp { left, op, right })
                }
                None => Ok(match left {
                    Expr::Call {
                        receiver: Some(r),
                        name,
                        args,
                    } => FilterNode::Call {
                        receiver: *r,
                        name,
                        args,
                    },
                    // Free function call at filter root (`hasTag("x")`).
                    // Surface as a Call against `note` so the evaluator
                    // can dispatch on `name`.
                    Expr::Call {
                        receiver: None,
                        name,
                        args,
                    } => FilterNode::Call {
                        receiver: Expr::NoteProp {
                            name: String::new(),
                        },
                        name,
                        args,
                    },
                    other => FilterNode::Truthy { expr: other },
                }),
            }
        }

        /// Top-level expression. Full precedence ladder mirroring
        /// JavaScript (which Obsidian's bases-language follows):
        ///   `or → and → unary-not → cmp → add → mul → unary-neg → postfix → primary`.
        ///
        /// The same operators also appear at the *filter* level via
        /// `parse_or`/`parse_and`/`parse_not`/`parse_cmp` — those
        /// produce `FilterNode`s; this ladder produces `Expr`s for
        /// use inside calls / args / arithmetic.
        fn parse_expr(&mut self) -> Result<Expr, BaseParseError> {
            self.parse_expr_or()
        }

        fn parse_expr_or(&mut self) -> Result<Expr, BaseParseError> {
            let mut left = self.parse_expr_and()?;
            while self.eat_logical_or() {
                let right = self.parse_expr_and()?;
                left = Expr::Binary {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_expr_and(&mut self) -> Result<Expr, BaseParseError> {
            let mut left = self.parse_expr_not()?;
            while self.eat_logical_and() {
                let right = self.parse_expr_not()?;
                left = Expr::Binary {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_expr_not(&mut self) -> Result<Expr, BaseParseError> {
            self.skip_ws();
            if self.peek() == Some(b'!') && self.src.get(self.pos + 1) != Some(&b'=') {
                self.pos += 1;
                let arg = self.parse_expr_not()?;
                return Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    arg: Box::new(arg),
                });
            }
            if self.eat_keyword(b"not") {
                let arg = self.parse_expr_not()?;
                return Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    arg: Box::new(arg),
                });
            }
            self.parse_expr_cmp()
        }

        fn parse_expr_cmp(&mut self) -> Result<Expr, BaseParseError> {
            let left = self.parse_add()?;
            self.skip_ws();
            let op = if self.src[self.pos..].starts_with(b"==") {
                self.pos += 2;
                Some(BinOp::Eq)
            } else if self.src[self.pos..].starts_with(b"!=") {
                self.pos += 2;
                Some(BinOp::Neq)
            } else if self.src[self.pos..].starts_with(b"<=") {
                self.pos += 2;
                Some(BinOp::Le)
            } else if self.src[self.pos..].starts_with(b">=") {
                self.pos += 2;
                Some(BinOp::Ge)
            } else if self.peek() == Some(b'<') {
                self.pos += 1;
                Some(BinOp::Lt)
            } else if self.peek() == Some(b'>') {
                self.pos += 1;
                Some(BinOp::Gt)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_add()?;
                    Ok(Expr::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    })
                }
                None => Ok(left),
            }
        }

        fn parse_add(&mut self) -> Result<Expr, BaseParseError> {
            let mut left = self.parse_mul()?;
            loop {
                self.skip_ws();
                let op = if self.peek() == Some(b'+') {
                    self.pos += 1;
                    BinOp::Add
                } else if self.peek() == Some(b'-') {
                    // Distinguish `a - b` from `-b`. We're past the
                    // unary site, so a bare `-` here is subtraction.
                    self.pos += 1;
                    BinOp::Sub
                } else {
                    break;
                };
                let right = self.parse_mul()?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_mul(&mut self) -> Result<Expr, BaseParseError> {
            let mut left = self.parse_unary()?;
            loop {
                self.skip_ws();
                let op = match self.peek() {
                    Some(b'*') => {
                        self.pos += 1;
                        BinOp::Mul
                    }
                    Some(b'/') => {
                        self.pos += 1;
                        BinOp::Div
                    }
                    Some(b'%') => {
                        self.pos += 1;
                        BinOp::Rem
                    }
                    _ => break,
                };
                let right = self.parse_unary()?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_unary(&mut self) -> Result<Expr, BaseParseError> {
            self.skip_ws();
            if self.peek() == Some(b'-')
                && self
                    .src
                    .get(self.pos + 1)
                    .is_some_and(|c| !c.is_ascii_digit())
            {
                self.pos += 1;
                let arg = self.parse_unary()?;
                return Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    arg: Box::new(arg),
                });
            }
            self.parse_postfix()
        }

        /// Parse a postfix expression — a primary followed by any
        /// chain of `.prop` accesses and `.method(args)` calls. Calls
        /// return `Expr::Call`, so they're usable in any expression
        /// context (args, RHS, arithmetic).
        fn parse_postfix(&mut self) -> Result<Expr, BaseParseError> {
            let mut expr = self.parse_primary()?;
            loop {
                self.skip_ws();
                if self.peek() == Some(b'.') {
                    self.pos += 1;
                    let ident = self.parse_ident()?;
                    self.skip_ws();
                    if self.peek() == Some(b'(') {
                        self.pos += 1;
                        let args = self.parse_args()?;
                        expr = Expr::Call {
                            receiver: Some(Box::new(expr)),
                            name: ident,
                            args,
                        };
                    } else {
                        // Property access — fold into the existing
                        // identifier path when the receiver carries
                        // one. Calls / arbitrary exprs become a Call
                        // with `name = "<prop>"` and zero args so the
                        // chain stays representable.
                        expr = match expr {
                            Expr::FileProp { name } => Expr::FileProp {
                                name: join_path(&name, &ident),
                            },
                            Expr::NoteProp { name } => Expr::NoteProp {
                                name: join_path(&name, &ident),
                            },
                            Expr::FormulaRef { name } => Expr::FormulaRef {
                                name: join_path(&name, &ident),
                            },
                            other => Expr::Call {
                                receiver: Some(Box::new(other)),
                                name: ident,
                                args: Vec::new(),
                            },
                        };
                    }
                } else {
                    break;
                }
            }
            Ok(expr)
        }

        fn parse_args(&mut self) -> Result<Vec<Expr>, BaseParseError> {
            let mut out = Vec::new();
            self.skip_ws();
            if self.peek() == Some(b')') {
                self.pos += 1;
                return Ok(out);
            }
            loop {
                let e = self.parse_expr()?;
                out.push(e);
                self.skip_ws();
                if self.eat(b",") {
                    continue;
                }
                if self.eat(b")") {
                    break;
                }
                return Err(BaseParseError::Expr("expected , or ) in args".into()));
            }
            Ok(out)
        }

        fn parse_primary(&mut self) -> Result<Expr, BaseParseError> {
            self.skip_ws();
            match self.peek() {
                Some(b'"' | b'\'') => self.parse_string(),
                Some(b'(') => {
                    self.pos += 1;
                    let e = self.parse_expr()?;
                    if !self.eat(b")") {
                        return Err(BaseParseError::Expr("expected )".into()));
                    }
                    Ok(e)
                }
                Some(c) if c.is_ascii_digit() => self.parse_number(),
                Some(c) if is_ident_start(c) => {
                    let ident = self.parse_ident()?;
                    match ident.as_str() {
                        "true" => Ok(Expr::Literal {
                            value: serde_json::Value::Bool(true),
                        }),
                        "false" => Ok(Expr::Literal {
                            value: serde_json::Value::Bool(false),
                        }),
                        "null" => Ok(Expr::Literal {
                            value: serde_json::Value::Null,
                        }),
                        "this" => Ok(Expr::This),
                        _ => {
                            // `ident(` → free function call.
                            self.skip_ws();
                            if self.peek() == Some(b'(') {
                                self.pos += 1;
                                let args = self.parse_args()?;
                                Ok(Expr::Call {
                                    receiver: None,
                                    name: ident,
                                    args,
                                })
                            } else {
                                Ok(expr_from_ident(ident))
                            }
                        }
                    }
                }
                Some(c) => Err(BaseParseError::Expr(format!(
                    "unexpected char {:?} at byte {}",
                    c as char, self.pos
                ))),
                None => Err(BaseParseError::Expr("unexpected end of input".into())),
            }
        }

        /// `||` literal or the bare `or` keyword (Obsidian-style).
        /// Keyword form must be followed by a non-ident-continue byte
        /// so we don't tear into identifiers like `order:`.
        fn eat_logical_or(&mut self) -> bool {
            self.skip_ws();
            if self.src[self.pos..].starts_with(b"||") {
                self.pos += 2;
                return true;
            }
            self.eat_keyword(b"or")
        }

        fn eat_logical_and(&mut self) -> bool {
            self.skip_ws();
            if self.src[self.pos..].starts_with(b"&&") {
                self.pos += 2;
                return true;
            }
            self.eat_keyword(b"and")
        }

        /// Consume `kw` iff it's at the cursor AND the next byte is
        /// not an ident-continue. Avoids mis-eating prefixes of real
        /// identifiers (`order`, `note`, etc).
        fn eat_keyword(&mut self, kw: &[u8]) -> bool {
            self.skip_ws();
            if !self.src[self.pos..].starts_with(kw) {
                return false;
            }
            match self.src.get(self.pos + kw.len()) {
                None => {
                    self.pos += kw.len();
                    true
                }
                Some(&c) if !is_ident_continue(c) => {
                    self.pos += kw.len();
                    true
                }
                _ => false,
            }
        }

        fn parse_ident(&mut self) -> Result<String, BaseParseError> {
            self.skip_ws();
            let start = self.pos;
            while let Some(c) = self.peek() {
                if is_ident_continue(c) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if start == self.pos {
                return Err(BaseParseError::Expr("expected identifier".into()));
            }
            Ok(std::str::from_utf8(&self.src[start..self.pos])
                .map_err(|e| BaseParseError::Expr(e.to_string()))?
                .to_string())
        }

        fn parse_string(&mut self) -> Result<Expr, BaseParseError> {
            let quote = self.src[self.pos];
            self.pos += 1;
            let start = self.pos;
            let mut buf = String::new();
            while self.pos < self.src.len() {
                let c = self.src[self.pos];
                if c == b'\\' && self.pos + 1 < self.src.len() {
                    let n = self.src[self.pos + 1];
                    buf.push(match n {
                        b'n' => '\n',
                        b't' => '\t',
                        b'"' => '"',
                        b'\'' => '\'',
                        b'\\' => '\\',
                        other => other as char,
                    });
                    self.pos += 2;
                    continue;
                }
                if c == quote {
                    self.pos += 1;
                    return Ok(Expr::Literal {
                        value: serde_json::Value::String(buf),
                    });
                }
                buf.push(c as char);
                self.pos += 1;
            }
            Err(BaseParseError::Expr(format!(
                "unterminated string starting at {start}"
            )))
        }

        fn parse_number(&mut self) -> Result<Expr, BaseParseError> {
            let start = self.pos;
            if self.peek() == Some(b'-') {
                self.pos += 1;
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() || c == b'.' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let raw = std::str::from_utf8(&self.src[start..self.pos])
                .map_err(|e| BaseParseError::Expr(e.to_string()))?;
            let n: f64 = raw
                .parse()
                .map_err(|e: std::num::ParseFloatError| BaseParseError::Expr(e.to_string()))?;
            Ok(Expr::Literal {
                value: serde_json::Number::from_f64(n)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number),
            })
        }
    }

    fn is_ident_start(c: u8) -> bool {
        c.is_ascii_alphabetic() || c == b'_'
    }

    fn is_ident_continue(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_'
    }

    fn expr_from_ident(ident: String) -> Expr {
        if ident == "file" {
            Expr::FileProp {
                name: String::new(),
            }
        } else if ident == "note" {
            Expr::NoteProp {
                name: String::new(),
            }
        } else if ident == "formula" {
            Expr::FormulaRef {
                name: String::new(),
            }
        } else {
            Expr::NoteProp { name: ident }
        }
    }

    fn join_path(prefix: &str, ident: &str) -> String {
        if prefix.is_empty() {
            ident.to_string()
        } else {
            format!("{prefix}.{ident}")
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const REP_BASE: &str = r#"
filters:
  and:
    - file.hasTag("book")
    - or:
        - status == "reading"
        - status == "to-read"
    - not: file.inFolder("Archive")
formulas:
  formattedPrice: 'toFixed(note.price, 2)'
  ageDays: '(now() - file.ctime) / 86400'
properties:
  note.author:
    displayName: "Author"
  status:
    displayName: "Status"
  formula.formattedPrice:
    displayName: "Price"
views:
  - type: table
    name: "All books"
    order:
      - file.name
      - status
      - formula.formattedPrice
    sort:
      - property: file.name
        direction: ASC
    limit: 100
  - type: board
    name: "By status"
    groupBy: status
  - type: gallery
    name: "Covers"
    image: note.cover
  - type: calendar
    name: "Due"
    dateProperty: note.due
  - type: list
    name: "Compact"
"#;

    #[test]
    fn parses_representative_base() {
        let parsed = parse(REP_BASE).expect("parse");
        assert_eq!(parsed.formulas.len(), 2);
        assert_eq!(parsed.properties.len(), 3);
        assert_eq!(parsed.views.len(), 5);
        // Filters round-trip into an And with 3 children.
        match parsed.global_filter {
            FilterNode::And { ref args } => assert_eq!(args.len(), 3),
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_parse_serialize_parse() {
        let parsed = parse(REP_BASE).expect("parse");
        let yaml = serialize(&parsed).expect("serialize");
        let reparsed = parse(&yaml).expect("reparse");
        assert_eq!(parsed.formulas, reparsed.formulas);
        assert_eq!(parsed.properties, reparsed.properties);
        assert_eq!(parsed.views.len(), reparsed.views.len());
        for (a, b) in parsed.views.iter().zip(reparsed.views.iter()) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.name, b.name);
            assert_eq!(a.order, b.order);
            assert_eq!(a.sort, b.sort);
            assert_eq!(a.limit, b.limit);
            assert_eq!(a.group_by, b.group_by);
        }
    }

    #[test]
    fn bare_comparison() {
        let f = expr_parser::parse_filter(r#"status == "done""#).unwrap();
        match f {
            FilterNode::Cmp { left, op, right } => {
                assert_eq!(op, CmpOp::Eq);
                assert_eq!(
                    left,
                    Expr::NoteProp {
                        name: "status".into()
                    }
                );
                assert_eq!(
                    right,
                    Expr::Literal {
                        value: serde_json::Value::String("done".into())
                    }
                );
            }
            other => panic!("expected Cmp, got {other:?}"),
        }
    }

    #[test]
    fn nested_and_or_not() {
        let yaml = r#"
filters:
  and:
    - or:
        - status == "a"
        - status == "b"
    - not: file.inFolder("X")
"#;
        let p = parse(yaml).unwrap();
        match p.global_filter {
            FilterNode::And { args } => {
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], FilterNode::Or { .. }));
                assert!(matches!(args[1], FilterNode::Not { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn function_call_with_string_and_number() {
        let f = expr_parser::parse_filter(r#"file.hasTag("book", 2)"#).unwrap();
        match f {
            FilterNode::Call {
                receiver,
                name,
                args,
            } => {
                assert_eq!(
                    receiver,
                    Expr::FileProp {
                        name: String::new()
                    }
                );
                assert_eq!(name, "hasTag");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], Expr::Literal { .. }));
                assert!(matches!(args[1], Expr::Literal { .. }));
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn formula_reference() {
        let f = expr_parser::parse_filter(r"formula.price > 10").unwrap();
        match f {
            FilterNode::Cmp { left, op, .. } => {
                assert_eq!(op, CmpOp::Gt);
                assert_eq!(
                    left,
                    Expr::FormulaRef {
                        name: "price".into()
                    }
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn all_view_kinds_parse() {
        let p = parse(REP_BASE).unwrap();
        let kinds: Vec<ViewKind> = p.views.iter().map(|v| v.kind.clone()).collect();
        assert!(kinds.contains(&ViewKind::Table));
        assert!(kinds.contains(&ViewKind::Board));
        assert!(kinds.contains(&ViewKind::Gallery));
        assert!(kinds.contains(&ViewKind::Calendar));
        assert!(kinds.contains(&ViewKind::List));
    }

    #[test]
    fn empty_yaml_is_ok() {
        let p = parse("").unwrap();
        assert_eq!(p.global_filter, FilterNode::None);
        assert!(p.views.is_empty());
    }
}

// ── Executor ─────────────────────────────────────────────────────────
//
// Phase 6 — runs a `ParsedBase` (or just a `ViewSpec`) over a page-set.
// Pages are presented as `BaseRow`s — minimal projection of a Knowledge
// `Page` that the executor needs (id, basename, frontmatter map).
// Reads pull `frontmatter_json` once and decode top-level keys into
// `serde_json::Value`.

/// Minimal page projection. The knowledge-ui layer constructs these
/// from `knowledge_proto::Page` values before feeding the executor.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseRow {
    pub page_id: uuid::Uuid,
    pub basename: String,
    /// Vault-relative path with extension (e.g. `Music/Charts.md`).
    /// Empty when unknown.
    pub path: String,
    /// Vault-relative parent folder; empty at root or when unknown.
    pub folder: String,
    /// File extension without the leading dot (e.g. `md`). Empty
    /// when unknown.
    pub ext: String,
    /// Union of frontmatter `tags` and inline `#tag` markers. Used
    /// by `file.tags` lookups so `.contains("chart")` works the
    /// way Obsidian does.
    pub tags: Vec<String>,
    /// Outgoing wikilink targets, as written (`Songs/Song A`, no
    /// brackets, alias or heading). Frontmatter links are collected
    /// by the constructors; body links are added with
    /// [`BaseRow::with_links`]. Backs `file.links` / `file.hasLink()`.
    pub links: Vec<String>,
    /// Top-level frontmatter map. Empty if the page has no
    /// frontmatter or it failed to decode.
    pub frontmatter: indexmap::IndexMap<String, serde_json::Value>,
}

impl BaseRow {
    /// Build from a `(page_id, basename, frontmatter_json)` triple.
    /// Path/folder/ext default to empty; tags are inferred from the
    /// frontmatter `tags` (or `tag`) key when present, links from any
    /// `[[wikilink]]` string in the frontmatter.
    pub fn from_parts(
        page_id: uuid::Uuid,
        basename: impl Into<String>,
        frontmatter_json: &str,
    ) -> Self {
        let frontmatter: indexmap::IndexMap<String, serde_json::Value> =
            serde_json::from_str(frontmatter_json).unwrap_or_default();
        let tags = tags_from_frontmatter(&frontmatter);
        let links = links_from_frontmatter(&frontmatter);
        Self {
            page_id,
            basename: basename.into(),
            path: String::new(),
            folder: String::new(),
            ext: String::new(),
            tags,
            links,
            frontmatter,
        }
    }

    /// Enriched constructor used by the vault bridge to populate
    /// `file.path` / `file.folder` / `file.ext` / `file.tags`
    /// lookups. `extra_tags` is merged into whatever's derived from
    /// the frontmatter (dedup, order-preserving) so inline `#tag`
    /// markers participate in `file.tags.contains(...)` filters.
    pub fn from_parts_full(
        page_id: uuid::Uuid,
        basename: impl Into<String>,
        rel_path: impl Into<String>,
        folder: impl Into<String>,
        ext: impl Into<String>,
        frontmatter_json: &str,
        extra_tags: &[String],
    ) -> Self {
        let frontmatter: indexmap::IndexMap<String, serde_json::Value> =
            serde_json::from_str(frontmatter_json).unwrap_or_default();
        let mut tags = tags_from_frontmatter(&frontmatter);
        for t in extra_tags {
            if !tags.iter().any(|x| x == t) {
                tags.push(t.clone());
            }
        }
        let links = links_from_frontmatter(&frontmatter);
        Self {
            page_id,
            basename: basename.into(),
            path: rel_path.into(),
            folder: folder.into(),
            ext: ext.into(),
            tags,
            links,
            frontmatter,
        }
    }

    /// Merge body wikilink targets into [`BaseRow::links`] (dedup,
    /// order-preserving). Targets are taken as written; brackets,
    /// `|alias` and `#heading` are stripped.
    #[must_use]
    pub fn with_links<S: AsRef<str>>(mut self, links: impl IntoIterator<Item = S>) -> Self {
        for l in links {
            let t = normalize_link_target(l.as_ref());
            if !t.is_empty() && !self.links.iter().any(|x| x == &t) {
                self.links.push(t);
            }
        }
        self
    }
}

fn tags_from_frontmatter(fm: &indexmap::IndexMap<String, serde_json::Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let t = s.trim().trim_start_matches('#').to_string();
        if !t.is_empty() && !out.iter().any(|x| x == &t) {
            out.push(t);
        }
    };
    for key in ["tags", "tag"] {
        match fm.get(key) {
            Some(serde_json::Value::Array(arr)) => {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        push(s);
                    }
                }
            }
            Some(serde_json::Value::String(s)) => {
                for piece in s.split([',', ' ']) {
                    push(piece);
                }
            }
            _ => {}
        }
    }
    out
}

/// Every `[[wikilink]]` anywhere in the frontmatter (strings, lists,
/// nested maps, and YAML's unquoted `[[x]]` nested-list form).
fn links_from_frontmatter(fm: &indexmap::IndexMap<String, serde_json::Value>) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(t) = link_of(v) {
            if !out.iter().any(|x| x == &t) {
                out.push(t);
            }
            return;
        }
        match v {
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => map.values().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for v in fm.values() {
        walk(v, &mut out);
    }
    out
}

/// `[[Target|alias]]`, `[[Target#Heading]]`, `Target.md` → `Target`.
/// Brackets are optional, so plain names and paths normalize too.
#[must_use]
pub fn normalize_link_target(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix("[[")
        .and_then(|r| r.strip_suffix("]]"))
        .unwrap_or(s);
    let s = s.split('|').next().unwrap_or("");
    let s = s.split('#').next().unwrap_or("");
    let s = s.trim().trim_start_matches('/');
    s.strip_suffix(".md").unwrap_or(s).to_string()
}

/// The link a value carries, if it is one: a `"[[Target]]"` string,
/// or the `[["Target"]]` nested list YAML makes of an unquoted
/// wikilink. Plain strings are not links.
fn link_of(v: &serde_json::Value) -> Option<String> {
    use serde_json::Value as V;
    match v {
        V::String(s) => {
            let t = s.trim();
            (t.starts_with("[[") && t.ends_with("]]") && t.len() > 4)
                .then(|| normalize_link_target(t))
                .filter(|t| !t.is_empty())
        }
        V::Array(outer) if outer.len() == 1 => match &outer[0] {
            V::Array(inner) if inner.len() == 1 => inner[0]
                .as_str()
                .map(normalize_link_target)
                .filter(|t| !t.is_empty()),
            _ => None,
        },
        _ => None,
    }
}

/// A link value or a plain string, as a link target — what
/// `hasLink(...)` / `.exists()` accept as their subject.
fn target_of(v: &serde_json::Value) -> Option<String> {
    link_of(v).or_else(|| {
        v.as_str()
            .map(normalize_link_target)
            .filter(|t| !t.is_empty())
    })
}

fn link_value(target: &str) -> serde_json::Value {
    serde_json::Value::String(format!("[[{target}]]"))
}

/// Result of running a view: rows grouped (or one bucket when no
/// `group_by`), in the order they should render.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutedView {
    /// `(bucket_label, rows)`. `bucket_label` is `""` when no
    /// `group_by` is configured.
    pub groups: Vec<(String, Vec<BaseRow>)>,
}

/// Run one `ViewSpec` over `rows`. Applies (in order):
/// 1. Global filter (AND-ed with view-scoped filter, if present).
/// 2. Sort by the view's `sort` keys, falling back to `basename`.
/// 3. Group by `group_by` (any column: frontmatter key, `file.*`,
///    `formula.*`). When `None`, one bucket labelled `""`.
/// 4. Limit (per-bucket).
///
/// Every row in `rows` is also the link-resolution universe, so
/// `note.song.key` can read a note the filter drops.
pub fn execute_view<I: IntoIterator<Item = BaseRow>>(
    base: &ParsedBase,
    view: &ViewSpec,
    rows: I,
) -> ExecutedView {
    let all: Vec<BaseRow> = rows.into_iter().collect();
    let cx = BaseEval::new(base, &all);
    let groups = cx
        .run(view)
        .into_iter()
        .map(|(label, idx)| (label, idx.into_iter().map(|i| all[i].clone()).collect()))
        .collect();
    ExecutedView { groups }
}

/// One view run and projected for a reader (an agent, a CLI): each
/// row's visible columns plus every formula's value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ViewTable {
    pub name: String,
    pub kind: String,
    /// The columns `values` carries, in order (the view's `order`,
    /// or `file.name` when it names none).
    pub columns: Vec<String>,
    pub groups: Vec<TableGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableGroup {
    /// `""` when the view has no `group_by`.
    pub label: String,
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableRow {
    pub path: String,
    pub name: String,
    /// Column → value, for every entry in [`ViewTable::columns`].
    pub values: indexmap::IndexMap<String, serde_json::Value>,
    /// Formula name → value, for every formula the base declares.
    pub formulas: indexmap::IndexMap<String, serde_json::Value>,
}

/// Run `view` over `rows` and project it into a [`ViewTable`].
#[must_use]
pub fn view_table(base: &ParsedBase, view: &ViewSpec, rows: &[BaseRow]) -> ViewTable {
    let cx = BaseEval::new(base, rows);
    let columns = if view.order.is_empty() {
        vec!["file.name".to_string()]
    } else {
        view.order.clone()
    };
    let groups = cx
        .run(view)
        .into_iter()
        .map(|(label, idx)| TableGroup {
            label,
            rows: idx
                .into_iter()
                .map(|i| {
                    let row = &rows[i];
                    TableRow {
                        path: row.path.clone(),
                        name: row.basename.clone(),
                        values: columns
                            .iter()
                            .map(|c| (c.clone(), cx.column(row, c)))
                            .collect(),
                        formulas: base
                            .formulas
                            .iter()
                            .map(|f| (f.name.clone(), cx.formula(&f.name, row)))
                            .collect(),
                    }
                })
                .collect(),
        })
        .collect();
    ViewTable {
        name: view.name.clone(),
        kind: view.kind.as_str().to_string(),
        columns,
        groups,
    }
}

fn value_to_label(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn is_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(m) => !m.is_empty(),
    }
}

fn cmp_values(l: &serde_json::Value, op: CmpOp, r: &serde_json::Value) -> bool {
    use serde_json::Value as V;
    match op {
        CmpOp::Contains => return value_contains(l, r),
        CmpOp::StartsWith => {
            if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
                return a.starts_with(b);
            }
            return false;
        }
        CmpOp::EndsWith => {
            if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
                return a.ends_with(b);
            }
            return false;
        }
        _ => {}
    }
    match (l, r) {
        (V::String(a), V::String(b)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::Neq => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
            _ => false,
        },
        (V::Number(a), V::Number(b)) => {
            let af = a.as_f64().unwrap_or(0.0);
            let bf = b.as_f64().unwrap_or(0.0);
            #[allow(clippy::float_cmp)]
            match op {
                CmpOp::Eq => af == bf,
                CmpOp::Neq => af != bf,
                CmpOp::Lt => af < bf,
                CmpOp::Le => af <= bf,
                CmpOp::Gt => af > bf,
                CmpOp::Ge => af >= bf,
                _ => false,
            }
        }
        (V::Bool(a), V::Bool(b)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::Neq => a != b,
            _ => false,
        },
        (V::Null, V::Null) => matches!(op, CmpOp::Eq),
        // Any other comparison involving Null is false except !=.
        (V::Null, _) | (_, V::Null) => matches!(op, CmpOp::Neq),
        _ => false,
    }
}

fn value_contains(haystack: &serde_json::Value, needle: &serde_json::Value) -> bool {
    use serde_json::Value as V;
    match haystack {
        V::String(s) => match needle {
            V::String(n) => s.contains(n.as_str()),
            _ => false,
        },
        V::Array(arr) => arr.iter().any(|v| v == needle),
        _ => false,
    }
}

// ── Expression evaluator ─────────────────────────────────────────────
//
// Walks the AST and produces a `serde_json::Value`. Unknown call
// names / unsupported constructs return `Value::Null` (never panic)
// — the bases language is open-ended via plugin extensions and we
// don't want one stray identifier to drop a whole view to zero rows.
//
// Evaluation runs against a [`BaseEval`]: the base's formulas plus the
// whole row set, indexed by path and basename so a link value
// (`"[[Song A]]"`) can be followed to the note it names.

/// How deep formula references (and the link hops inside them) may
/// nest before the evaluator gives up with `null`. Cycles are caught
/// earlier by the in-progress marker in [`BaseEval::formula`]; this
/// bounds long link-follow chains and the stack.
const MAX_EVAL_DEPTH: u8 = 16;

/// One memoized formula value for one row.
enum FormulaSlot {
    /// Being evaluated right now: a reference back to it is a cycle.
    InProgress,
    Done(serde_json::Value),
}

/// Evaluation context for one base over one row set.
// t[impl vault.bases.links]
pub struct BaseEval<'a> {
    rows: &'a [BaseRow],
    /// Lower-cased vault path without `.md` → row index.
    by_path: std::collections::HashMap<String, usize>,
    /// Lower-cased basename → row index, or `None` when two notes
    /// share the basename (an ambiguous `[[Name]]` resolves to nothing).
    by_name: std::collections::HashMap<String, Option<usize>>,
    /// Formula name → parsed expression (or its parse error).
    formulas: std::collections::HashMap<String, Result<Expr, String>>,
    /// The base's top-level `filters:`; [`FilterNode::None`] without a base.
    global_filter: FilterNode,
    depth: std::cell::Cell<u8>,
    /// `(formula, row index)` → value, so each formula is evaluated at
    /// most once per row however often it is referenced: evaluation is
    /// linear in formulas × rows, even for a caller-written base whose
    /// formulas fan out (`a: b + b`, `b: c + c`, …).
    memo: std::cell::RefCell<std::collections::HashMap<(String, usize), FormulaSlot>>,
    /// Formula evaluations actually run (memo misses); a cost probe
    /// for the tests.
    formula_evals: std::cell::Cell<u64>,
}

impl<'a> BaseEval<'a> {
    /// Index `rows` for link resolution and parse `base`'s formulas.
    #[must_use]
    pub fn new(base: &ParsedBase, rows: &'a [BaseRow]) -> Self {
        let mut cx = Self::over(rows);
        cx.global_filter = base.global_filter.clone();
        cx.formulas = base
            .formulas
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    expr_parser::parse_expression(&f.expression).map_err(|e| e.to_string()),
                )
            })
            .collect();
        cx
    }

    /// A context with rows but no formulas.
    #[must_use]
    pub fn over(rows: &'a [BaseRow]) -> Self {
        let mut by_path = std::collections::HashMap::new();
        let mut by_name = std::collections::HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            if !r.path.is_empty() {
                by_path
                    .entry(normalize_link_target(&r.path).to_lowercase())
                    .or_insert(i);
            }
            by_name
                .entry(r.basename.to_lowercase())
                .and_modify(|slot| *slot = None)
                .or_insert(Some(i));
        }
        Self {
            rows,
            by_path,
            by_name,
            formulas: std::collections::HashMap::new(),
            global_filter: FilterNode::None,
            depth: std::cell::Cell::new(0),
            memo: std::cell::RefCell::new(std::collections::HashMap::new()),
            formula_evals: std::cell::Cell::new(0),
        }
    }

    /// The row a link target names: an exact vault path first, then
    /// the basename (Obsidian's shortest-unique-name links).
    #[must_use]
    pub fn resolve(&self, target: &str) -> Option<&'a BaseRow> {
        let key = normalize_link_target(target).to_lowercase();
        if key.is_empty() {
            return None;
        }
        self.by_path
            .get(&key)
            .or_else(|| {
                if key.contains('/') {
                    None
                } else {
                    self.by_name.get(&key).and_then(Option::as_ref)
                }
            })
            .map(|&i| &self.rows[i])
    }

    /// `row`'s index when it is one of this context's rows (by
    /// address, O(1)); `None` for a row from elsewhere.
    fn slot_of(&self, row: &BaseRow) -> Option<usize> {
        let size = std::mem::size_of::<BaseRow>();
        let start = self.rows.as_ptr() as usize;
        let at = std::ptr::from_ref(row) as usize;
        let off = at.checked_sub(start)?;
        (size > 0 && off % size == 0 && off / size < self.rows.len()).then_some(off / size)
    }

    /// Filter, sort, group and limit `view` (AND-ed with the base's
    /// global filter). Returns row indices into the row set.
    fn run(&self, view: &ViewSpec) -> Vec<(String, Vec<usize>)> {
        let base_filter = self.global_filter.clone();
        let combined = match &view.filter {
            Some(vf) if !matches!(base_filter, FilterNode::None) => FilterNode::And {
                args: vec![base_filter, vf.clone()],
            },
            Some(vf) => vf.clone(),
            None => base_filter,
        };
        let mut keep: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.filter_matches(&combined, &self.rows[i]))
            .collect();
        keep.sort_by(|&a, &b| self.compare_rows(&self.rows[a], &self.rows[b], &view.sort));

        let mut groups: Vec<(String, Vec<usize>)> = if let Some(key) = view.group_by.as_deref() {
            let mut buckets: indexmap::IndexMap<String, Vec<usize>> = indexmap::IndexMap::new();
            for i in keep {
                let label = value_to_label(&self.column(&self.rows[i], key));
                buckets.entry(label).or_default().push(i);
            }
            // Stable: insertion order is the order labels were first
            // observed. Caller can reorder via post-processing.
            buckets.into_iter().collect()
        } else {
            vec![(String::new(), keep)]
        };
        if let Some(limit) = view.limit {
            for (_, v) in &mut groups {
                v.truncate(limit as usize);
            }
        }
        groups
    }

    /// Whether `node` holds for `row`.
    #[must_use]
    pub fn filter_matches(&self, node: &FilterNode, row: &BaseRow) -> bool {
        match node {
            FilterNode::None => true,
            FilterNode::And { args } => args.iter().all(|n| self.filter_matches(n, row)),
            FilterNode::Or { args } => args.iter().any(|n| self.filter_matches(n, row)),
            FilterNode::Not { arg } => !self.filter_matches(arg, row),
            FilterNode::Truthy { expr } => is_truthy(&self.eval(expr, row)),
            FilterNode::Cmp { left, op, right } => {
                cmp_values(&self.eval(left, row), *op, &self.eval(right, row))
            }
            // Unified dispatch — wrap into `Expr::Call` and reuse the
            // expression evaluator so `contains` / `hasTag` /
            // `startsWith` / … behave identically whether they appear
            // as a filter root or inside a nested expression.
            FilterNode::Call {
                receiver,
                name,
                args,
            } => {
                let call = Expr::Call {
                    receiver: Some(Box::new(receiver.clone())),
                    name: name.clone(),
                    args: args.clone(),
                };
                is_truthy(&self.eval(&call, row))
            }
        }
    }

    /// The value of one view column (`file.*`, `formula.*`, `note.*`
    /// or a bare frontmatter key) for `row`.
    #[must_use]
    pub fn column(&self, row: &BaseRow, column: &str) -> serde_json::Value {
        if column == "basename" {
            return serde_json::Value::String(row.basename.clone());
        }
        let expr = if let Some(name) = column.strip_prefix("file.") {
            Expr::FileProp { name: name.into() }
        } else if let Some(name) = column.strip_prefix("formula.") {
            Expr::FormulaRef { name: name.into() }
        } else {
            Expr::NoteProp {
                name: column.strip_prefix("note.").unwrap_or(column).into(),
            }
        };
        self.eval(&expr, row)
    }

    /// One formula's value for `row`; `null` for an unknown or
    /// unparseable formula, one that refers back to itself (directly
    /// or through others), or one nested deeper than [`MAX_EVAL_DEPTH`].
    ///
    /// Values are memoized per `(formula, row)`, so a formula runs at
    /// most once per row. A formula met again while it is still being
    /// evaluated is a cycle and reads as `null`.
    #[must_use]
    pub fn formula(&self, name: &str, row: &BaseRow) -> serde_json::Value {
        let Some(Ok(expr)) = self.formulas.get(name) else {
            return serde_json::Value::Null;
        };
        let Some(i) = self.slot_of(row) else {
            // Not one of our rows (a caller-built row): no memo key.
            // Still bounded by the depth guard.
            self.formula_evals.set(self.formula_evals.get() + 1);
            return self.nested(|| self.eval(expr, row));
        };
        let key = (name.to_owned(), i);
        match self.memo.borrow().get(&key) {
            Some(FormulaSlot::Done(v)) => return v.clone(),
            Some(FormulaSlot::InProgress) => return serde_json::Value::Null,
            None => {}
        }
        self.memo
            .borrow_mut()
            .insert(key.clone(), FormulaSlot::InProgress);
        self.formula_evals.set(self.formula_evals.get() + 1);
        let v = self.nested(|| self.eval(expr, row));
        self.memo
            .borrow_mut()
            .insert(key, FormulaSlot::Done(v.clone()));
        v
    }

    /// Run `f` one level deeper; `null` past [`MAX_EVAL_DEPTH`].
    fn nested(&self, f: impl FnOnce() -> serde_json::Value) -> serde_json::Value {
        let d = self.depth.get();
        if d >= MAX_EVAL_DEPTH {
            return serde_json::Value::Null;
        }
        self.depth.set(d + 1);
        let v = f();
        self.depth.set(d);
        v
    }

    /// Evaluate `expr` against `row`.
    #[must_use]
    pub fn eval(&self, expr: &Expr, row: &BaseRow) -> serde_json::Value {
        use serde_json::Value as V;
        match expr {
            Expr::Literal { value } => value.clone(),
            // `this` — full templated bases (Project Hierarchy, etc.)
            // aren't wired up yet. Returning Null lets `this.up == foo`
            // style predicates parse-and-degrade-to-empty cleanly.
            // FUTURE: thread "current page" into the context and resolve.
            Expr::This => V::Null,
            Expr::FileProp { name } => self.file_prop(name, row),
            Expr::NoteProp { name } => self.note_prop(name, row),
            Expr::FormulaRef { name } => {
                let mut parts = name.split('.');
                let head = parts.next().unwrap_or("");
                if head.is_empty() {
                    return V::Null;
                }
                let v = self.formula(head, row);
                self.walk(v, parts)
            }
            Expr::Unary { op, arg } => {
                let v = self.eval(arg, row);
                match op {
                    UnaryOp::Neg => match v.as_f64() {
                        Some(f) => json_num(-f),
                        None => V::Null,
                    },
                    UnaryOp::Not => V::Bool(!is_truthy(&v)),
                }
            }
            Expr::Binary { op, left, right } => {
                let cmp = |op: CmpOp| {
                    V::Bool(cmp_values(&self.eval(left, row), op, &self.eval(right, row)))
                };
                let arith = |f: fn(f64, f64) -> f64| {
                    arith_vals(&self.eval(left, row), &self.eval(right, row), f)
                };
                match op {
                    BinOp::And => {
                        if !is_truthy(&self.eval(left, row)) {
                            return V::Bool(false);
                        }
                        V::Bool(is_truthy(&self.eval(right, row)))
                    }
                    BinOp::Or => {
                        if is_truthy(&self.eval(left, row)) {
                            return V::Bool(true);
                        }
                        V::Bool(is_truthy(&self.eval(right, row)))
                    }
                    BinOp::Eq => cmp(CmpOp::Eq),
                    BinOp::Neq => cmp(CmpOp::Neq),
                    BinOp::Lt => cmp(CmpOp::Lt),
                    BinOp::Le => cmp(CmpOp::Le),
                    BinOp::Gt => cmp(CmpOp::Gt),
                    BinOp::Ge => cmp(CmpOp::Ge),
                    BinOp::Add => {
                        let l = self.eval(left, row);
                        let r = self.eval(right, row);
                        // String concat if either side is a string (matches
                        // JS/Obsidian); otherwise numeric addition.
                        if matches!(l, V::String(_)) || matches!(r, V::String(_)) {
                            V::String(format!("{}{}", value_to_string(&l), value_to_string(&r)))
                        } else {
                            arith_vals(&l, &r, |a, b| a + b)
                        }
                    }
                    BinOp::Sub => arith(|a, b| a - b),
                    BinOp::Mul => arith(|a, b| a * b),
                    BinOp::Div => arith(|a, b| a / b),
                    BinOp::Rem => arith(|a, b| a % b),
                }
            }
            Expr::Call {
                receiver,
                name,
                args,
            } => self.call(receiver.as_deref(), name, args, row),
        }
    }

    /// Walk the remaining `.seg` path segments from `v`.
    fn walk<'s>(
        &self,
        mut v: serde_json::Value,
        segs: impl Iterator<Item = &'s str>,
    ) -> serde_json::Value {
        for seg in segs {
            v = self.member(&v, seg);
        }
        v
    }

    /// `v.seg`: a map key, a list/string `length`, a property of the
    /// note a link names (link-follow), or — on a plain list — the
    /// member of each element.
    fn member(&self, v: &serde_json::Value, seg: &str) -> serde_json::Value {
        use serde_json::Value as V;
        if let Some(target) = link_of(v) {
            return match self.resolve(&target) {
                Some(linked) => self.nested(|| self.note_prop(seg, linked)),
                None => V::Null,
            };
        }
        match v {
            V::Object(map) => map.get(seg).cloned().unwrap_or(V::Null),
            V::String(s) if seg == "length" => json_num(s.chars().count() as f64),
            V::Array(items) if seg == "length" => json_num(items.len() as f64),
            V::Array(items) => V::Array(items.iter().map(|i| self.member(i, seg)).collect()),
            _ => V::Null,
        }
    }

    fn file_prop(&self, name: &str, row: &BaseRow) -> serde_json::Value {
        use serde_json::Value as V;
        let mut parts = name.split('.');
        let head = parts.next().unwrap_or("");
        let v = match head {
            // `file` itself with no member access — used only as a
            // receiver for method calls; return Null so a bare `file`
            // doesn't accidentally pass a Truthy filter.
            "" => V::Null,
            "name" | "basename" => V::String(row.basename.clone()),
            "path" => {
                if row.path.is_empty() {
                    V::Null
                } else {
                    V::String(row.path.clone())
                }
            }
            "folder" => V::String(row.folder.clone()),
            "ext" => {
                if row.ext.is_empty() {
                    V::Null
                } else {
                    V::String(row.ext.clone())
                }
            }
            "tags" => V::Array(row.tags.iter().map(|t| V::String(t.clone())).collect()),
            // Each outgoing link as a link value, so `file.links` can be
            // followed (`file.links.status`) or counted.
            "links" => V::Array(row.links.iter().map(String::as_str).map(link_value).collect()),
            // FUTURE: file.size / file.mtime / file.ctime /
            // file.embeds — we don't carry that data on `BaseRow` yet.
            _ => V::Null,
        };
        self.walk(v, parts)
    }

    fn note_prop(&self, name: &str, row: &BaseRow) -> serde_json::Value {
        if name.is_empty() {
            // bare `note` — used as a method receiver; not a value on
            // its own.
            return serde_json::Value::Null;
        }
        // Dotted paths walk maps and follow links (`note.song.key`).
        let mut parts = name.split('.');
        let head = parts.next().unwrap_or("");
        let v = row
            .frontmatter
            .get(head)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        self.walk(v, parts)
    }

    /// Whether link targets `a` and `b` name the same note: the same
    /// resolved row, or (unresolved) the same normalized text.
    fn same_target(&self, a: &str, b: &str) -> bool {
        match (self.resolve(a), self.resolve(b)) {
            (Some(x), Some(y)) => std::ptr::eq(x, y),
            _ => normalize_link_target(a).eq_ignore_ascii_case(&normalize_link_target(b)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn call(
        &self,
        receiver: Option<&Expr>,
        name: &str,
        args: &[Expr],
        row: &BaseRow,
    ) -> serde_json::Value {
        use serde_json::Value as V;
        if receiver.is_none() && name == "if" && args.len() == 3 {
            // Lazy: only evaluate the chosen branch.
            return if is_truthy(&self.eval(&args[0], row)) {
                self.eval(&args[1], row)
            } else {
                self.eval(&args[2], row)
            };
        }
        let arg_vals: Vec<V> = args.iter().map(|a| self.eval(a, row)).collect();
        if let Some(recv) = receiver {
            let recv_v = self.eval(recv, row);
            match name {
                "contains" if arg_vals.len() == 1 => V::Bool(value_contains(&recv_v, &arg_vals[0])),
                "startsWith" if arg_vals.len() == 1 => {
                    match (recv_v.as_str(), arg_vals[0].as_str()) {
                        (Some(a), Some(b)) => V::Bool(a.starts_with(b)),
                        _ => V::Bool(false),
                    }
                }
                "endsWith" if arg_vals.len() == 1 => match (recv_v.as_str(), arg_vals[0].as_str())
                {
                    (Some(a), Some(b)) => V::Bool(a.ends_with(b)),
                    _ => V::Bool(false),
                },
                "isEmpty" => V::Bool(match &recv_v {
                    V::Null => true,
                    V::String(s) => s.is_empty(),
                    V::Array(a) => a.is_empty(),
                    V::Object(m) => m.is_empty(),
                    _ => false,
                }),
                "length" => match &recv_v {
                    V::String(s) => json_num(s.chars().count() as f64),
                    V::Array(a) => json_num(a.len() as f64),
                    _ => V::Null,
                },
                "hasTag" if arg_vals.len() == 1 => {
                    // Receiver is typically `file` / `note` /
                    // `file.tags`. Normalize to an array of strings
                    // and contains-check the needle, stripping any
                    // leading `#` on either side.
                    let tags = receiver_tags(&recv_v, row);
                    let needle = match arg_vals[0].as_str() {
                        Some(s) => s.trim_start_matches('#').to_string(),
                        None => return V::Bool(false),
                    };
                    V::Bool(tags.iter().any(|t| t == &needle))
                }
                // `file.hasLink(x)` — the note links to `x` (a link
                // value, a name or a path), in its body or frontmatter.
                // On a link receiver it asks the linked note.
                "hasLink" if arg_vals.len() == 1 => {
                    let Some(needle) = target_of(&arg_vals[0]) else {
                        return V::Bool(false);
                    };
                    match self.subject_of(recv, &recv_v, row) {
                        Some(subject) => V::Bool(
                            subject
                                .links
                                .iter()
                                .any(|l| self.same_target(l, &needle)),
                        ),
                        None => V::Bool(false),
                    }
                }
                // `file.exists()` is always true for the row itself;
                // `note.song.exists()` asks whether the link resolves.
                "exists" if arg_vals.is_empty() => {
                    V::Bool(self.subject_of(recv, &recv_v, row).is_some())
                }
                "hasProperty" if arg_vals.len() == 1 => match arg_vals[0].as_str() {
                    Some(key) => V::Bool(
                        self.subject_of(recv, &recv_v, row)
                            .is_some_and(|s| s.frontmatter.contains_key(key)),
                    ),
                    None => V::Bool(false),
                },
                "asLink" if arg_vals.is_empty() => match self.subject_of(recv, &recv_v, row) {
                    Some(s) => link_value(&s.basename),
                    None => V::Null,
                },
                // Obsidian `file.inFolder("Path")` — the note's path is under
                // the given folder (or a subfolder of it).
                "inFolder" if arg_vals.len() == 1 => {
                    let needle = arg_vals[0].as_str().unwrap_or("").trim_matches('/');
                    let path = row.path.trim_start_matches('/');
                    V::Bool(
                        needle.is_empty()
                            || path == needle
                            || path.starts_with(&format!("{needle}/")),
                    )
                }
                "floor" => recv_v.as_f64().map_or(V::Null, |f| json_num(f.floor())),
                "round" => recv_v.as_f64().map_or(V::Null, |f| json_num(f.round())),
                "ceil" => recv_v.as_f64().map_or(V::Null, |f| json_num(f.ceil())),
                // FUTURE: real date/number formatting via chrono +
                // a locale-aware number formatter. v1 = pass-through
                // stringification.
                "format" => V::String(value_to_string(&recv_v)),
                // FUTURE: proper date truncation / coercion.
                "date" => recv_v,
                // FUTURE: lambda-style higher-order calls. Args are
                // mini-expressions referencing an implicit element;
                // we'd need a scoped row binding to evaluate them.
                "filter" | "map" | "reduce" => V::Null,
                // `link("X").key` parses as a zero-arg call named `key`
                // on a non-path receiver: treat it as member access.
                _ if args.is_empty() => self.member(&recv_v, name),
                _ => V::Null,
            }
        } else {
            match name {
                "today" | "now" => {
                    // ISO-8601 UTC. Tests that need determinism can
                    // avoid these by sticking to scalar filters.
                    // FUTURE: inject a `Clock` trait for deterministic
                    // tests.
                    V::String(chrono::Utc::now().to_rfc3339())
                }
                "date" if arg_vals.len() == 1 => arg_vals.into_iter().next().unwrap(),
                "number" if arg_vals.len() == 1 => match &arg_vals[0] {
                    V::Number(_) => arg_vals.into_iter().next().unwrap(),
                    V::String(s) => s.parse::<f64>().map(json_num).unwrap_or(V::Null),
                    V::Bool(b) => json_num(if *b { 1.0 } else { 0.0 }),
                    _ => V::Null,
                },
                // `link("Songs/Song A")` → a link value.
                "link" if !arg_vals.is_empty() => {
                    target_of(&arg_vals[0]).map_or(V::Null, |t| link_value(&t))
                }
                "list" => match arg_vals.as_slice() {
                    [V::Array(_)] => arg_vals.into_iter().next().unwrap(),
                    _ => V::Array(arg_vals),
                },
                "min" if arg_vals.len() == 2 => num_pair(&arg_vals[0], &arg_vals[1])
                    .map_or(V::Null, |(a, b)| json_num(a.min(b))),
                "max" if arg_vals.len() == 2 => num_pair(&arg_vals[0], &arg_vals[1])
                    .map_or(V::Null, |(a, b)| json_num(a.max(b))),
                "hasTag" if arg_vals.len() == 1 => {
                    let needle = match arg_vals[0].as_str() {
                        Some(s) => s.trim_start_matches('#').to_string(),
                        None => return V::Bool(false),
                    };
                    V::Bool(row.tags.iter().any(|t| t == &needle))
                }
                // `transpose_key(key, offset)` — a key name moved by
                // semitones (`4`, `"+4"`, `"-4"`) in its own spelling, or
                // replaced by an absolute key (`"D"`). See `music_key`.
                "transpose_key" if arg_vals.len() == 2 => {
                    transpose_key_value(&arg_vals[0], &arg_vals[1])
                }
                _ => V::Null,
            }
        }
    }

    /// The note a receiver names, if it exists: the row itself for bare
    /// `file` / `note`, else the note a link value resolves to.
    fn subject_of<'r>(
        &'r self,
        recv: &Expr,
        recv_v: &serde_json::Value,
        row: &'r BaseRow,
    ) -> Option<&'r BaseRow> {
        if matches!(recv, Expr::FileProp { name } | Expr::NoteProp { name } if name.is_empty()) {
            return Some(row);
        }
        target_of(recv_v).and_then(|t| self.resolve(&t))
    }

    fn compare_rows(&self, a: &BaseRow, b: &BaseRow, sort: &[SortKey]) -> std::cmp::Ordering {
        for key in sort {
            let ord = compare_values(&self.column(a, &key.property), &self.column(b, &key.property));
            let ord = match key.direction {
                SortDir::Asc => ord,
                SortDir::Desc => ord.reverse(),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        a.basename.cmp(&b.basename)
    }
}

/// Context-free evaluation (no formulas, no link universe) — what the
/// executor did before [`BaseEval`]; kept for the unit tests.
#[cfg(test)]
fn eval_expr(expr: &Expr, row: &BaseRow) -> serde_json::Value {
    BaseEval::over(&[]).eval(expr, row)
}

fn arith_vals(
    l: &serde_json::Value,
    r: &serde_json::Value,
    op: impl FnOnce(f64, f64) -> f64,
) -> serde_json::Value {
    match (l.as_f64(), r.as_f64()) {
        (Some(a), Some(b)) => json_num(op(a, b)),
        _ => serde_json::Value::Null,
    }
}

fn json_num(f: f64) -> serde_json::Value {
    serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Display string for one `order` column of a row — resolves `file.*`
/// props and bare / `note.` frontmatter keys, then stringifies (arrays comma-joined). `formula.*` needs
/// the base: use [`BaseEval::column`] for that.
#[must_use]
pub fn cell_value(row: &BaseRow, column: &str) -> String {
    let v = BaseEval::over(std::slice::from_ref(row)).column(row, column);
    match &v {
        serde_json::Value::Array(items) => items
            .iter()
            .map(value_to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => value_to_string(&v),
    }
}

/// The `transpose_key(key, offset)` formula over JSON values.
///
/// A missing or empty offset means "no change", so a setlist entry
/// without `key_offset` shows the song's key. A non-integer number or
/// an unparseable key/offset is `null`, like every other type mismatch
/// in this evaluator. An absolute key offset is returned as written,
/// whatever `key` holds.
fn transpose_key_value(key: &serde_json::Value, offset: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value as V;
    let offset = match offset {
        V::Null => "0".to_string(),
        V::String(s) if s.trim().is_empty() => "0".to_string(),
        V::String(s) => s.clone(),
        V::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(f)) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
            _ => return V::Null,
        },
        _ => return V::Null,
    };
    let key = key.as_str().unwrap_or("");
    music_key::transpose_key(key, &offset).map_or(V::Null, V::String)
}

fn num_pair(a: &serde_json::Value, b: &serde_json::Value) -> Option<(f64, f64)> {
    Some((a.as_f64()?, b.as_f64()?))
}

fn receiver_tags(recv: &serde_json::Value, row: &BaseRow) -> Vec<String> {
    use serde_json::Value as V;
    match recv {
        V::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim_start_matches('#').to_string()))
            .collect(),
        // Bare `file` / `note` evaluate to Null in our model; fall
        // back to the row's union tag set.
        V::Null => row.tags.clone(),
        V::String(s) => vec![s.trim_start_matches('#').to_string()],
        _ => Vec::new(),
    }
}

fn compare_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    use serde_json::Value as V;
    match (a, b) {
        (V::Null, V::Null) => std::cmp::Ordering::Equal,
        (V::Null, _) => std::cmp::Ordering::Less,
        (_, V::Null) => std::cmp::Ordering::Greater,
        (V::String(x), V::String(y)) => x.cmp(y),
        (V::Number(x), V::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&y.as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal),
        (V::Bool(x), V::Bool(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod executor_tests {
    use super::*;
    use uuid::Uuid;

    fn row(name: &str, fm: serde_json::Value) -> BaseRow {
        BaseRow::from_parts(Uuid::new_v4(), name, &fm.to_string())
    }

    fn task(name: &str, status: &str) -> BaseRow {
        row(
            name,
            serde_json::json!({ "kind": "task", "status": status }),
        )
    }

    fn note(name: &str) -> BaseRow {
        row(name, serde_json::json!({ "kind": "note" }))
    }

    fn task_kanban_base() -> ParsedBase {
        ParsedBase {
            folder: String::new(),
            tags: vec![],
            global_filter: FilterNode::Cmp {
                left: Expr::NoteProp {
                    name: "kind".into(),
                },
                op: CmpOp::Eq,
                right: Expr::Literal {
                    value: serde_json::json!("task"),
                },
            },
            formulas: vec![],
            properties: vec![],
            views: vec![ViewSpec {
                kind: ViewKind::Board,
                name: "Kanban".into(),
                filter: None,
                order: vec!["status".into()],
                sort: vec![SortKey {
                    property: "basename".into(),
                    direction: SortDir::Asc,
                }],
                limit: None,
                group_by: Some("status".into()),
                extras: serde_json::Value::Null,
            }],
        }
    }

    #[test]
    fn filters_by_kind_and_groups_by_status() {
        let base = task_kanban_base();
        let view = &base.views[0];
        let rows = vec![
            task("Buy capacitors", "todo"),
            task("Solder header", "in_progress"),
            task("Test rig", "done"),
            note("Meeting"),
        ];
        let out = execute_view(&base, view, rows);
        let labels: Vec<&str> = out.groups.iter().map(|(k, _)| k.as_str()).collect();
        assert!(labels.contains(&"todo"));
        assert!(labels.contains(&"in_progress"));
        assert!(labels.contains(&"done"));
        // Meeting (kind=note) is filtered out → no "" bucket.
        assert!(!labels.contains(&""));
        let total: usize = out.groups.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn ungrouped_returns_single_bucket() {
        let mut base = task_kanban_base();
        base.views[0].group_by = None;
        let view = &base.views[0];
        let out = execute_view(
            &base,
            view,
            vec![task("a", "todo"), task("b", "in_progress")],
        );
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].0, "");
        assert_eq!(out.groups[0].1.len(), 2);
    }

    #[test]
    fn limit_truncates_per_bucket() {
        let mut base = task_kanban_base();
        base.views[0].limit = Some(1);
        let view = &base.views[0];
        let out = execute_view(
            &base,
            view,
            vec![
                task("a", "todo"),
                task("b", "todo"),
                task("c", "in_progress"),
            ],
        );
        for (_label, rows) in &out.groups {
            assert!(rows.len() <= 1);
        }
    }

    #[test]
    fn sort_desc_reverses() {
        let base = ParsedBase {
            global_filter: FilterNode::None,
            formulas: vec![],
            properties: vec![],
            folder: String::new(),
            tags: vec![],
            views: vec![ViewSpec {
                kind: ViewKind::List,
                name: "All".into(),
                filter: None,
                order: vec![],
                sort: vec![SortKey {
                    property: "basename".into(),
                    direction: SortDir::Desc,
                }],
                limit: None,
                group_by: None,
                extras: serde_json::Value::Null,
            }],
        };
        let out = execute_view(&base, &base.views[0], vec![note("a"), note("c"), note("b")]);
        let names: Vec<&str> = out.groups[0]
            .1
            .iter()
            .map(|r| r.basename.as_str())
            .collect();
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    #[test]
    fn contains_filters_by_tag() {
        // Mirrors a real Charts.base: filter pages whose
        // `file.tags` array contains the literal `"chart"`.
        let base = ParsedBase {
            global_filter: FilterNode::None,
            formulas: vec![],
            properties: vec![],
            folder: String::new(),
            tags: vec![],
            views: vec![ViewSpec {
                kind: ViewKind::List,
                name: "All Charts".into(),
                filter: Some(FilterNode::And {
                    args: vec![FilterNode::Call {
                        receiver: Expr::FileProp {
                            name: "tags".into(),
                        },
                        name: "contains".into(),
                        args: vec![Expr::Literal {
                            value: serde_json::json!("chart"),
                        }],
                    }],
                }),
                order: vec![],
                sort: vec![],
                limit: None,
                group_by: None,
                extras: serde_json::Value::Null,
            }],
        };
        let rows = vec![
            row("Alpha", serde_json::json!({ "tags": ["chart"] })),
            row("Beta", serde_json::json!({ "tags": ["demo"] })),
            row("Gamma", serde_json::json!({ "tags": [] })),
        ];
        let out = execute_view(&base, &base.views[0], rows);
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].1.len(), 1);
        assert_eq!(out.groups[0].1[0].basename, "Alpha");
    }

    #[test]
    fn string_concat_via_add() {
        let r = row("x", serde_json::json!({}));
        let expr = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Literal {
                value: serde_json::json!("a"),
            }),
            right: Box::new(Expr::Literal {
                value: serde_json::json!("b"),
            }),
        };
        assert_eq!(eval_expr(&expr, &r), serde_json::json!("ab"));
    }

    #[test]
    fn boolean_short_circuit() {
        let r = row("x", serde_json::json!({}));
        // `false && undefined_thing()` — RHS would eval to Null,
        // but short-circuit returns Bool(false) before touching it.
        let expr = Expr::Binary {
            op: BinOp::And,
            left: Box::new(Expr::Literal {
                value: serde_json::json!(false),
            }),
            right: Box::new(Expr::Call {
                receiver: None,
                name: "undefined_thing".into(),
                args: vec![],
            }),
        };
        assert_eq!(eval_expr(&expr, &r), serde_json::json!(false));
    }

    #[test]
    fn if_returns_branch() {
        let r = row("x", serde_json::json!({}));
        let expr = Expr::Call {
            receiver: None,
            name: "if".into(),
            args: vec![
                Expr::Literal {
                    value: serde_json::json!(true),
                },
                Expr::Literal {
                    value: serde_json::json!("yes"),
                },
                Expr::Literal {
                    value: serde_json::json!("no"),
                },
            ],
        };
        assert_eq!(eval_expr(&expr, &r), serde_json::json!("yes"));
    }

    fn transpose_call(key: Expr, offset: Expr) -> Expr {
        Expr::Call {
            receiver: None,
            name: "transpose_key".into(),
            args: vec![key, offset],
        }
    }

    fn lit(v: serde_json::Value) -> Expr {
        Expr::Literal { value: v }
    }

    #[test]
    fn transpose_key_formula_values() {
        use serde_json::json;
        let r = row("x", json!({}));
        let cases = [
            (json!("f#m"), json!(2), json!("g#m")),
            (json!("D"), json!("+4"), json!("F#")),
            (json!("D"), json!("-4"), json!("Bb")),
            (json!("Eb"), json!(3), json!("Gb")),
            (json!("Bb minor"), json!(-1), json!("A minor")),
            (json!("G"), json!(4.0), json!("B")),
            // Absolute key: passed through unchanged.
            (json!("C"), json!("D"), json!("D")),
            (json!(null), json!("Ebm"), json!("Ebm")),
            // No offset: the key itself.
            (json!("Ab"), json!(null), json!("Ab")),
            (json!("Ab"), json!(""), json!("Ab")),
            // Bad input: null.
            (json!("H"), json!(2), json!(null)),
            (json!("Cmaj7"), json!(2), json!(null)),
            (json!("C"), json!(1.5), json!(null)),
            (json!("C"), json!("four"), json!(null)),
            (json!("C"), json!(true), json!(null)),
            (json!(null), json!(2), json!(null)),
        ];
        for (key, offset, want) in cases {
            let expr = transpose_call(lit(key.clone()), lit(offset.clone()));
            assert_eq!(eval_expr(&expr, &r), want, "transpose_key({key}, {offset})");
        }
        // Arity is checked: one argument is not a call we know.
        let one = Expr::Call {
            receiver: None,
            name: "transpose_key".into(),
            args: vec![lit(json!("C"))],
        };
        assert_eq!(eval_expr(&one, &r), json!(null));
    }

    #[test]
    fn transpose_key_reads_properties_in_a_filter() {
        use serde_json::json;
        // The setlist shape: each use has the song key (until link
        // follow lands, a plain property) and a per-use offset or key.
        let rows = vec![
            row(
                "1 Opener",
                json!({ "kind": "use", "key": "D", "key_offset": 4 }),
            ),
            row(
                "2 Ballad",
                json!({ "kind": "use", "key": "Bb", "key_offset": "D" }),
            ),
            row(
                "3 Closer",
                json!({ "kind": "use", "key": "f#m", "key_offset": "+2" }),
            ),
            row("4 Default", json!({ "kind": "use", "key": "F#" })),
        ];
        let f = expr_parser::parse_filter(r#"transpose_key(key, key_offset) == "F#""#).unwrap();
        let hits: Vec<_> = rows
            .iter()
            .filter(|r| BaseEval::over(&[]).filter_matches(&f, r))
            .map(|r| r.basename.clone())
            .collect();
        assert_eq!(hits, vec!["1 Opener", "4 Default"]);

        let f = expr_parser::parse_filter(r#"transpose_key(note.key, note.key_offset) == "g#m""#)
            .unwrap();
        assert!(BaseEval::over(&[]).filter_matches(&f, &rows[2]));
        let f = expr_parser::parse_filter(r#"transpose_key(key, key_offset) == "D""#).unwrap();
        assert!(BaseEval::over(&[]).filter_matches(&f, &rows[1]));

        // With link-follow: the use links its song, the key lives there.
        let song = page("Songs/Night Drive.md", json!({ "key": "D" }), &[]);
        let usage = page(
            "Gigs/1 Opener.md",
            json!({ "song": "[[Night Drive]]", "key_offset": 4 }),
            &[],
        );
        let all = vec![song, usage.clone()];
        let f = expr_parser::parse_filter(r#"transpose_key(note.song.key, key_offset) == "F#""#)
            .unwrap();
        assert!(BaseEval::over(&all).filter_matches(&f, &usage));
    }

    // ── Link functions, link-follow and formulas ─────────────────

    /// A row at a vault path, with frontmatter and body links.
    fn page(path: &str, fm: serde_json::Value, body_links: &[&str]) -> BaseRow {
        let basename = path
            .rsplit('/')
            .next()
            .unwrap()
            .trim_end_matches(".md")
            .to_string();
        let folder = path.rsplit_once('/').map_or("", |(f, _)| f).to_string();
        BaseRow::from_parts_full(
            Uuid::new_v4(),
            basename,
            path,
            folder,
            "md",
            &fm.to_string(),
            &[],
        )
        .with_links(body_links.iter().copied())
    }

    /// The gigs recipe: songs carry `key`, a gig links its song in
    /// frontmatter and its venue in the body.
    fn gig_vault() -> Vec<BaseRow> {
        vec![
            page(
                "Songs/Song A.md",
                serde_json::json!({ "key": "G", "bpm": 120 }),
                &[],
            ),
            page(
                "Songs/Song B.md",
                serde_json::json!({ "key": "D", "bpm": 90 }),
                &[],
            ),
            page("Venues/The Hall.md", serde_json::json!({ "city": "Austin" }), &[]),
            page(
                "Gigs/Friday.md",
                serde_json::json!({ "song": "[[Song A]]", "fee": 400, "setlist": ["[[Song A]]", "[[Songs/Song B|B]]"] }),
                &["[[The Hall#Load-in]]"],
            ),
            page(
                "Gigs/Saturday.md",
                // Unquoted YAML wikilink → nested list; still a link.
                serde_json::json!({ "song": [["Song B"]], "fee": 250 }),
                &[],
            ),
            page(
                "Gigs/Sunday.md",
                serde_json::json!({ "song": "[[Song Nobody Wrote]]", "fee": 0 }),
                &[],
            ),
        ]
    }

    fn eval_src(cx: &BaseEval<'_>, src: &str, row: &BaseRow) -> serde_json::Value {
        let expr = expr_parser::parse_expression(src).unwrap();
        cx.eval(&expr, row)
    }

    fn filter_src(cx: &BaseEval<'_>, src: &str, row: &BaseRow) -> bool {
        let f = expr_parser::parse_filter(src).unwrap();
        cx.filter_matches(&f, row)
    }

    #[test]
    fn link_targets_normalize() {
        assert_eq!(normalize_link_target("[[Songs/Song A|A]]"), "Songs/Song A");
        assert_eq!(normalize_link_target("[[Song A#Bridge]]"), "Song A");
        assert_eq!(normalize_link_target("Songs/Song A.md"), "Songs/Song A");
        assert_eq!(normalize_link_target("  Song A "), "Song A");
    }

    #[test]
    fn file_links_lists_frontmatter_and_body_links() {
        let rows = gig_vault();
        let cx = BaseEval::over(&rows);
        let friday = &rows[3];
        assert_eq!(
            eval_src(&cx, "file.links", friday),
            serde_json::json!(["[[Song A]]", "[[Songs/Song B]]", "[[The Hall]]"])
        );
        assert_eq!(eval_src(&cx, "file.links.length", friday), serde_json::json!(3.0));
        // Nested-list YAML links count as links too.
        assert_eq!(
            eval_src(&cx, "file.links", &rows[4]),
            serde_json::json!(["[[Song B]]"])
        );
        // A note with no links has an empty list, not null.
        assert_eq!(eval_src(&cx, "file.links", &rows[0]), serde_json::json!([]));
    }

    #[test]
    // t[verify vault.bases.links]
    fn has_link_matches_by_name_path_and_link_value() {
        let rows = gig_vault();
        let cx = BaseEval::over(&rows);
        let friday = &rows[3];
        // Frontmatter link, by bare name, by link value, by full path.
        assert!(filter_src(&cx, r#"file.hasLink("Song A")"#, friday));
        assert!(filter_src(&cx, r#"file.hasLink(link("Song A"))"#, friday));
        assert!(filter_src(&cx, r#"file.hasLink("Songs/Song A.md")"#, friday));
        // Aliased + path-qualified link resolves to the same note as a
        // bare-name needle.
        assert!(filter_src(&cx, r#"file.hasLink("Song B")"#, friday));
        // Body link with a heading.
        assert!(filter_src(&cx, r#"file.hasLink("The Hall")"#, friday));
        assert!(!filter_src(&cx, r#"file.hasLink("Song A")"#, &rows[4]));
        assert!(!filter_src(&cx, r#"file.hasLink("Elsewhere")"#, friday));
        // Unresolved links still match by their text.
        assert!(filter_src(&cx, r#"file.hasLink("Song Nobody Wrote")"#, &rows[5]));
        // `not` composes.
        assert!(filter_src(&cx, r#"!file.hasLink("Song B")"#, &rows[5]));
    }

    #[test]
    fn exists_resolves_links() {
        let rows = gig_vault();
        let cx = BaseEval::over(&rows);
        assert!(filter_src(&cx, "file.exists()", &rows[0]));
        assert!(filter_src(&cx, "note.song.exists()", &rows[3]));
        assert!(filter_src(&cx, "song.exists()", &rows[4]));
        assert!(!filter_src(&cx, "note.song.exists()", &rows[5]));
        // No such property → nothing to resolve.
        assert!(!filter_src(&cx, "note.venue.exists()", &rows[3]));
        assert!(filter_src(&cx, r#"file.hasProperty("fee")"#, &rows[3]));
        assert!(!filter_src(&cx, r#"file.hasProperty("key")"#, &rows[3]));
        assert_eq!(
            eval_src(&cx, "note.song.asLink()", &rows[3]),
            serde_json::json!("[[Song A]]")
        );
    }

    #[test]
    fn link_follow_reads_the_linked_notes_property() {
        let rows = gig_vault();
        let cx = BaseEval::over(&rows);
        assert_eq!(eval_src(&cx, "note.song.key", &rows[3]), serde_json::json!("G"));
        assert_eq!(eval_src(&cx, "song.bpm", &rows[4]), serde_json::json!(90));
        // Broken link → null, never a panic.
        assert_eq!(eval_src(&cx, "note.song.key", &rows[5]), serde_json::Value::Null);
        // A list of links maps member access over each.
        assert_eq!(
            eval_src(&cx, "note.setlist.key", &rows[3]),
            serde_json::json!(["G", "D"])
        );
        assert_eq!(eval_src(&cx, r#"link("Song B").key"#, &rows[0]), serde_json::json!("D"));
        // …and in filters.
        assert!(filter_src(&cx, r#"note.song.key == "G""#, &rows[3]));
        assert!(!filter_src(&cx, r#"note.song.key == "G""#, &rows[4]));
        assert!(filter_src(&cx, "song.bpm > 100", &rows[3]));
    }

    #[test]
    fn link_follow_chains_and_terminates_on_cycles() {
        // A path is finite, so a link cycle can only be walked as far as
        // the expression says — each hop is one more `.next`.
        let rows = vec![
            page("A.md", serde_json::json!({ "next": "[[B]]", "name": "a" }), &[]),
            page("B.md", serde_json::json!({ "next": "[[A]]", "name": "b" }), &[]),
        ];
        let cx = BaseEval::over(&rows);
        assert_eq!(eval_src(&cx, "next.name", &rows[0]), serde_json::json!("b"));
        assert_eq!(eval_src(&cx, "next.next.name", &rows[0]), serde_json::json!("a"));
        let deep = std::iter::repeat_n("next", 40).collect::<Vec<_>>().join(".");
        assert_eq!(eval_src(&cx, &deep, &rows[0]), serde_json::json!("[[A]]"));
    }

    const GIGS_BASE: &str = r#"
filters:
  and:
    - file.inFolder("Gigs")
formulas:
  cut: "fee * 0.1"
  net: "fee - formula.cut"
  song_key: "note.song.key"
  label: "file.name + ' in ' + if(note.song.exists(), note.song.key, '?')"
  loop_a: "formula.loop_b + 1"
  loop_b: "formula.loop_a + 1"
views:
  - type: table
    name: Gigs in G
    filters:
      and:
        - 'note.song.key == "G"'
    order:
      - file.name
      - fee
      - formula.net
  - type: table
    name: By net
    order:
      - file.name
      - formula.song_key
    sort:
      - property: formula.net
        direction: ASC
  - type: board
    name: By key
    groupBy:
      property: formula.song_key
"#;

    #[test]
    fn fanned_out_formulas_run_once_per_row() {
        // f0 = f1 + f1, f1 = f2 + f2, … f14 = f15 + f15, f15 = 1.
        // Without a memo that is 2^16 - 1 evaluations per row; with
        // it, one per formula per row.
        let mut yaml = String::from("formulas:\n");
        for i in 0..15 {
            yaml.push_str(&format!("  f{i}: \"formula.f{n} + formula.f{n}\"\n", n = i + 1));
        }
        yaml.push_str("  f15: \"1\"\nviews:\n  - type: table\n    name: All\n    order:\n      - formula.f0\n");
        let base = parse(&yaml).unwrap();
        let rows = vec![
            page("A.md", serde_json::json!({}), &[]),
            page("B.md", serde_json::json!({}), &[]),
        ];
        let cx = BaseEval::new(&base, &rows);
        for r in &rows {
            assert_eq!(cx.formula("f0", r), serde_json::json!(32768.0));
        }
        assert_eq!(cx.formula_evals.get(), 16 * 2);
        // Asking again is free.
        let _ = cx.column(&rows[0], "formula.f0");
        assert_eq!(cx.formula_evals.get(), 16 * 2);

        // A fanned-out cycle is still null, and still cheap.
        let mut yaml = String::from("formulas:\n");
        for i in 0..12 {
            yaml.push_str(&format!(
                "  c{i}: \"formula.c{n} + formula.c{n}\"\n",
                n = (i + 1) % 12
            ));
        }
        let base = parse(&yaml).unwrap();
        let cx = BaseEval::new(&base, &rows);
        assert_eq!(cx.formula("c0", &rows[0]), serde_json::Value::Null);
        assert_eq!(cx.formula_evals.get(), 12, "{}", cx.formula_evals.get());
    }

    #[test]
    fn an_ambiguous_basename_resolves_to_nothing() {
        let rows = vec![
            page("Songs/Intro.md", serde_json::json!({ "key": "C" }), &[]),
            page("Drafts/Intro.md", serde_json::json!({ "key": "D" }), &[]),
            page("Songs/Outro.md", serde_json::json!({ "key": "E" }), &[]),
        ];
        let cx = BaseEval::over(&rows);
        // Two notes are called Intro: a bare [[Intro]] names neither.
        assert!(cx.resolve("Intro").is_none());
        assert!(cx.resolve("[[Intro]]").is_none());
        // A path still picks one.
        assert_eq!(cx.resolve("Songs/Intro").unwrap().path, "Songs/Intro.md");
        assert_eq!(cx.resolve("[[Drafts/Intro]]").unwrap().path, "Drafts/Intro.md");
        // A unique basename resolves as before.
        assert_eq!(cx.resolve("Outro").unwrap().path, "Songs/Outro.md");
    }

    #[test]
    fn formulas_evaluate_reference_each_other_and_follow_links() {
        let base = parse(GIGS_BASE).unwrap();
        let rows = gig_vault();
        let cx = BaseEval::new(&base, &rows);
        let friday = &rows[3];
        assert_eq!(cx.formula("cut", friday), serde_json::json!(40.0));
        assert_eq!(cx.formula("net", friday), serde_json::json!(360.0));
        assert_eq!(cx.formula("song_key", friday), serde_json::json!("G"));
        assert_eq!(cx.formula("label", friday), serde_json::json!("Friday in G"));
        assert_eq!(cx.formula("label", &rows[5]), serde_json::json!("Sunday in ?"));
        // A cycle is null, not a stack overflow.
        assert_eq!(cx.formula("loop_a", friday), serde_json::Value::Null);
        assert_eq!(cx.formula("missing", friday), serde_json::Value::Null);
        assert_eq!(cx.column(friday, "formula.net"), serde_json::json!(360.0));
    }

    #[test]
    fn views_filter_sort_and_group_on_links_and_formulas() {
        let base = parse(GIGS_BASE).unwrap();
        let rows = gig_vault();

        let in_g = execute_view(&base, &base.views[0], rows.clone());
        let names: Vec<&str> = in_g.groups[0].1.iter().map(|r| r.basename.as_str()).collect();
        assert_eq!(names, ["Friday"]);

        let by_net = execute_view(&base, &base.views[1], rows.clone());
        let names: Vec<&str> = by_net.groups[0].1.iter().map(|r| r.basename.as_str()).collect();
        // Ascending net — the reverse of the basename fallback order.
        assert_eq!(names, ["Sunday", "Saturday", "Friday"]);

        let by_key = execute_view(&base, &base.views[2], rows.clone());
        let labels: Vec<&str> = by_key.groups.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["G", "D", ""]);
    }

    #[test]
    fn view_table_projects_columns_and_formula_values() {
        let base = parse(GIGS_BASE).unwrap();
        let rows = gig_vault();
        let table = view_table(&base, &base.views[0], &rows);
        assert_eq!(table.name, "Gigs in G");
        assert_eq!(table.kind, "table");
        assert_eq!(table.columns, ["file.name", "fee", "formula.net"]);
        assert_eq!(table.groups.len(), 1);
        let row = &table.groups[0].rows[0];
        assert_eq!(row.path, "Gigs/Friday.md");
        assert_eq!(row.values["file.name"], serde_json::json!("Friday"));
        assert_eq!(row.values["fee"], serde_json::json!(400));
        assert_eq!(row.values["formula.net"], serde_json::json!(360.0));
        assert_eq!(row.formulas["song_key"], serde_json::json!("G"));
        assert_eq!(row.formulas.len(), base.formulas.len());
        // A view with no `order` still names each row.
        let bare = ViewSpec {
            order: vec![],
            ..base.views[1].clone()
        };
        assert_eq!(view_table(&base, &bare, &rows).columns, ["file.name"]);
    }
}
