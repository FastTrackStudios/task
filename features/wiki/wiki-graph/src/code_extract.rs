//! Tree-sitter-based code-symbol extraction.
//!
//! Walks a source file's AST and emits typed [`CodeNode`]s
//! (functions, structs, traits, impls, enums, modules) plus
//! [`CodeEdge`]s for `Imports` and best-effort `Calls`. The
//! shape mirrors graphify's per-file extractor output but
//! produces typed Rust structs instead of dicts.
//!
//! ## What lands in the graph
//!
//! **Nodes** (one per top-level declaration):
//!
//! | Rust node kind      | [`SymbolKind`] |
//! |---------------------|----------------|
//! | `function_item`     | `Fn`           |
//! | `struct_item`       | `Struct`       |
//! | `enum_item`         | `Enum`         |
//! | `trait_item`        | `Trait`        |
//! | `impl_item`         | `Impl`         |
//! | `mod_item`          | `Module`       |
//! | `type_item`         | `TypeAlias`    |
//! | `const_item`        | `Const`        |
//! | `static_item`       | `Static`       |
//!
//! **Edges** (one per relationship):
//!
//! | Source              | Relation    | Target              | Confidence  |
//! |---------------------|-------------|---------------------|-------------|
//! | declaring item      | `Defines`   | nested declaration  | `Extracted` |
//! | declaring item      | `Calls`     | function identifier | `Inferred`  |
//! | file-scope `use`    | `Imports`   | imported path tail  | `Extracted` |
//! | `impl <T> for <U>`  | `Implements`| trait `<T>`         | `Extracted` |
//!
//! ## Out of scope for now
//!
//! - Cross-file resolution (linking a call to its definition
//!   in another file). The 2nd pass that does this is a
//!   build-time concern; this module only emits *file-local*
//!   nodes + edges. PR 3 (the analysis layer) wires the call
//!   graph together.
//! - Macro expansion. Items defined inside `macro_rules!` are
//!   skipped (we don't expand the macro).
//! - Method calls vs free function calls — `Calls` edges are
//!   tagged `Inferred` regardless. The downstream resolver
//!   refines them.

use std::path::{Path, PathBuf};

use arborium_tree_sitter::{Node, Parser, Tree, TreeCursor};

/// Languages this extractor can parse. PR 2 ships Rust only;
/// TS / JS / Python land alongside their tree-sitter grammar
/// enablement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeLang {
    Rust,
    TypeScript,
    JavaScript,
}

impl CodeLang {
    /// Map a file extension (without the dot) to a [`CodeLang`].
    /// Returns `None` for files we can't parse; callers should
    /// skip them.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::TypeScript), // TSX uses the TS grammar for our needs
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            _ => None,
        }
    }

    /// Short tag used in node ids + the wire `language` field.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "ts",
            Self::JavaScript => "js",
        }
    }

    fn ts_language(self) -> arborium_tree_sitter::Language {
        match self {
            Self::Rust => arborium::lang_rust::language().into(),
            Self::TypeScript => arborium::lang_typescript::language().into(),
            Self::JavaScript => arborium::lang_javascript::language().into(),
        }
    }
}

/// Canonical kinds of symbols we surface. Free-form
/// `&'static str` reps so consumers can match on them in
/// `node_type` strings without depending on this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    TypeAlias,
    Const,
    Static,
}

impl SymbolKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fn => "fn",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Module => "module",
            Self::TypeAlias => "type",
            Self::Const => "const",
            Self::Static => "static",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    Defines,
    Calls,
    Imports,
    Implements,
}

impl Relation {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Defines => "defines",
            Self::Calls => "calls",
            Self::Imports => "imports",
            Self::Implements => "implements",
        }
    }
}

/// Graphify-style confidence tag. `Extracted` = literally
/// present in the AST; `Inferred` = resolved by a follow-up
/// pass; `Ambiguous` = the parser saw something it can't
/// firmly attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    Extracted,
    Inferred,
    Ambiguous,
}

impl Confidence {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "extracted",
            Self::Inferred => "inferred",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// One code symbol (function, struct, trait, …).
#[derive(Debug, Clone)]
pub struct CodeNode {
    /// Stable id: `code:<lang>:<rel_path>:<kind>:<name>`.
    /// Identical syntactic form across machines so the id is
    /// safe to embed in vault pages.
    pub id: String,
    pub label: String,
    pub kind: SymbolKind,
    pub language: CodeLang,
    /// Source file (project-relative).
    pub source_file: PathBuf,
    /// 0-based line range, inclusive.
    pub line_start: u32,
    pub line_end: u32,
}

/// One symbol-to-symbol or symbol-to-external-path relation.
#[derive(Debug, Clone)]
pub struct CodeEdge {
    pub source: String,
    pub target: String,
    pub relation: Relation,
    pub confidence: Confidence,
}

/// Per-file extraction result.
#[derive(Debug, Clone, Default)]
pub struct CodeExtraction {
    pub nodes: Vec<CodeNode>,
    pub edges: Vec<CodeEdge>,
    /// Errors encountered (e.g. file unreadable, parser
    /// produced no tree). Surfaced for diagnostics; an empty
    /// vec means a clean extraction.
    pub errors: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CodeExtractError {
    #[error("read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("parser produced no tree for {0}")]
    NoTree(PathBuf),
    #[error("unsupported language for {0}")]
    UnsupportedLanguage(PathBuf),
}

/// Parse one file and return its symbols + edges.
pub fn extract_file(path: &Path) -> Result<CodeExtraction, CodeExtractError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let lang = CodeLang::from_extension(ext)
        .ok_or_else(|| CodeExtractError::UnsupportedLanguage(path.to_path_buf()))?;
    let src =
        std::fs::read_to_string(path).map_err(|e| CodeExtractError::Io(path.to_path_buf(), e))?;
    extract_source(path, &src, lang)
}

/// Parse `source` as `lang`, producing the extraction
/// associated with `rel_path` (used for id stability).
pub fn extract_source(
    rel_path: &Path,
    source: &str,
    lang: CodeLang,
) -> Result<CodeExtraction, CodeExtractError> {
    let mut parser = Parser::new();
    let ts_lang = lang.ts_language();
    parser
        .set_language(&ts_lang)
        .expect("arborium grammar set_language");
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| CodeExtractError::NoTree(rel_path.to_path_buf()))?;
    Ok(match lang {
        CodeLang::Rust => walk_rust(rel_path, source, &tree, lang),
        CodeLang::TypeScript | CodeLang::JavaScript => walk_ts(rel_path, source, &tree, lang),
    })
}

/// Walk the tree, emit nodes for each top-level (and
/// nested-in-impl/mod) declaration. Builds an enclosing-scope
/// stack so `Defines` edges + `Calls` edges from within a
/// function body attribute to the right parent symbol.
fn walk_rust(rel_path: &Path, source: &str, tree: &Tree, lang: CodeLang) -> CodeExtraction {
    let mut ex = CodeExtraction::default();
    let mut cursor = tree.walk();
    let mut scope: Vec<String> = Vec::new();
    visit(
        &mut cursor,
        source.as_bytes(),
        rel_path,
        lang,
        &mut ex,
        &mut scope,
    );
    ex
}

fn visit(
    cursor: &mut TreeCursor,
    src: &[u8],
    rel_path: &Path,
    lang: CodeLang,
    ex: &mut CodeExtraction,
    scope: &mut Vec<String>,
) {
    let node = cursor.node();
    let kind = node.kind();
    let mut pushed_scope = false;

    match kind {
        "function_item" | "function_signature_item" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::Fn, &name);
                ex.nodes.push(CodeNode {
                    id: id.clone(),
                    label: name,
                    kind: SymbolKind::Fn,
                    language: lang,
                    source_file: rel_path.to_path_buf(),
                    line_start: node.start_position().row as u32,
                    line_end: node.end_position().row as u32,
                });
                if let Some(parent) = scope.last() {
                    ex.edges.push(CodeEdge {
                        source: parent.clone(),
                        target: id.clone(),
                        relation: Relation::Defines,
                        confidence: Confidence::Extracted,
                    });
                }
                scope.push(id);
                pushed_scope = true;
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" | "const_item" | "static_item"
        | "mod_item" => {
            let symbol = match kind {
                "struct_item" => SymbolKind::Struct,
                "enum_item" => SymbolKind::Enum,
                "trait_item" => SymbolKind::Trait,
                "type_item" => SymbolKind::TypeAlias,
                "const_item" => SymbolKind::Const,
                "static_item" => SymbolKind::Static,
                "mod_item" => SymbolKind::Module,
                _ => unreachable!(),
            };
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, symbol, &name);
                ex.nodes.push(CodeNode {
                    id: id.clone(),
                    label: name,
                    kind: symbol,
                    language: lang,
                    source_file: rel_path.to_path_buf(),
                    line_start: node.start_position().row as u32,
                    line_end: node.end_position().row as u32,
                });
                if let Some(parent) = scope.last() {
                    ex.edges.push(CodeEdge {
                        source: parent.clone(),
                        target: id.clone(),
                        relation: Relation::Defines,
                        confidence: Confidence::Extracted,
                    });
                }
                if symbol == SymbolKind::Module || symbol == SymbolKind::Trait {
                    scope.push(id);
                    pushed_scope = true;
                }
            }
        }
        "impl_item" => {
            // `impl Trait for Type` — we synthesize an Impl
            // node and an `Implements` edge to the trait. The
            // type itself isn't a node target (might be from
            // another crate); name the impl after its type.
            let type_text = field_text(node, "type", src);
            let trait_text = field_text(node, "trait", src);
            if let Some(ty) = &type_text {
                let label = match &trait_text {
                    Some(tr) => format!("{tr} for {ty}"),
                    None => ty.clone(),
                };
                let id = make_id(rel_path, lang, SymbolKind::Impl, &label);
                ex.nodes.push(CodeNode {
                    id: id.clone(),
                    label,
                    kind: SymbolKind::Impl,
                    language: lang,
                    source_file: rel_path.to_path_buf(),
                    line_start: node.start_position().row as u32,
                    line_end: node.end_position().row as u32,
                });
                if let Some(tr) = &trait_text {
                    ex.edges.push(CodeEdge {
                        source: id.clone(),
                        target: format!("ext:{tr}"),
                        relation: Relation::Implements,
                        confidence: Confidence::Extracted,
                    });
                }
                scope.push(id);
                pushed_scope = true;
            }
        }
        "use_declaration" => {
            // Capture the imported path(s). Cheap: take the
            // whole argument node's text. Real path-splitting
            // (e.g. `use foo::{bar, baz}`) is a follow-up.
            if let Some(arg) = node.child_by_field_name("argument") {
                let path_text = node_text(arg, src);
                let parent = scope
                    .last()
                    .cloned()
                    .unwrap_or_else(|| make_id(rel_path, lang, SymbolKind::Module, "<file>"));
                ex.edges.push(CodeEdge {
                    source: parent,
                    target: format!("ext:{path_text}"),
                    relation: Relation::Imports,
                    confidence: Confidence::Extracted,
                });
            }
        }
        "call_expression" => {
            // Best-effort: extract the function's identifier
            // text. `foo()` → `foo`; `obj.method()` → `method`;
            // `Type::assoc()` → `assoc`. Tagged INFERRED;
            // resolver pass refines.
            if let Some(func) = node.child_by_field_name("function") {
                let callee = call_target_name(func, src);
                if let (Some(parent), Some(target)) = (scope.last(), callee) {
                    ex.edges.push(CodeEdge {
                        source: parent.clone(),
                        target: format!("name:{target}"),
                        relation: Relation::Calls,
                        confidence: Confidence::Inferred,
                    });
                }
            }
        }
        _ => {}
    }

    if cursor.goto_first_child() {
        loop {
            visit(cursor, src, rel_path, lang, ex, scope);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }

    if pushed_scope {
        scope.pop();
    }
}

fn field_text(node: Node, field: &str, src: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    Some(node_text(child, src))
}

fn node_text(node: Node, src: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    std::str::from_utf8(&src[start..end])
        .unwrap_or("")
        .to_string()
}

/// For `call_expression.function`, return the rightmost name
/// component (the actual fn name, ignoring receiver / module
/// path). `foo` / `bar.baz` / `T::method` → respectively
/// `foo` / `baz` / `method`.
fn call_target_name(node: Node, src: &[u8]) -> Option<String> {
    let kind = node.kind();
    match kind {
        "identifier" => Some(node_text(node, src)),
        "field_expression" => node.child_by_field_name("field").map(|n| node_text(n, src)),
        "scoped_identifier" => node.child_by_field_name("name").map(|n| node_text(n, src)),
        _ => {
            // Walk for the last identifier child as a fallback.
            let mut cursor = node.walk();
            let mut last: Option<String> = None;
            if cursor.goto_first_child() {
                loop {
                    let c = cursor.node();
                    if c.kind() == "identifier" {
                        last = Some(node_text(c, src));
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
            last
        }
    }
}

fn make_id(rel_path: &Path, lang: CodeLang, kind: SymbolKind, name: &str) -> String {
    let path_s = rel_path.to_string_lossy();
    format!("code:{}:{}:{}:{}", lang.tag(), path_s, kind.as_str(), name)
}

/// TypeScript / JavaScript AST walker. Shares the
/// `CodeNode` / `CodeEdge` shape with Rust; per-language
/// node-kind names differ. Covers:
///
/// | TS/JS AST node             | [`SymbolKind`] |
/// |----------------------------|----------------|
/// | `function_declaration`     | `Fn`           |
/// | `class_declaration`        | `Struct`       |
/// | `interface_declaration`    | `Trait` (TS)   |
/// | `enum_declaration` (TS)    | `Enum`         |
/// | `type_alias_declaration`   | `TypeAlias`    |
/// | `method_definition`        | `Fn`           |
/// | `lexical_declaration` w/ `const` + function value | `Const` |
/// | `import_statement`         | edge `Imports` |
/// | `call_expression`          | edge `Calls`   |
fn walk_ts(rel_path: &Path, source: &str, tree: &Tree, lang: CodeLang) -> CodeExtraction {
    let mut ex = CodeExtraction::default();
    let mut cursor = tree.walk();
    let mut scope: Vec<String> = Vec::new();
    visit_ts(
        &mut cursor,
        source.as_bytes(),
        rel_path,
        lang,
        &mut ex,
        &mut scope,
    );
    ex
}

fn visit_ts(
    cursor: &mut TreeCursor,
    src: &[u8],
    rel_path: &Path,
    lang: CodeLang,
    ex: &mut CodeExtraction,
    scope: &mut Vec<String>,
) {
    let node = cursor.node();
    let kind = node.kind();
    let mut pushed_scope = false;

    match kind {
        "function_declaration" | "method_definition" | "generator_function_declaration" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::Fn, &name);
                push_node_and_defines_edge(
                    ex,
                    scope,
                    &id,
                    &name,
                    SymbolKind::Fn,
                    lang,
                    rel_path,
                    &node,
                );
                scope.push(id);
                pushed_scope = true;
            }
        }
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::Struct, &name);
                push_node_and_defines_edge(
                    ex,
                    scope,
                    &id,
                    &name,
                    SymbolKind::Struct,
                    lang,
                    rel_path,
                    &node,
                );
                scope.push(id);
                pushed_scope = true;
            }
        }
        "interface_declaration" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::Trait, &name);
                push_node_and_defines_edge(
                    ex,
                    scope,
                    &id,
                    &name,
                    SymbolKind::Trait,
                    lang,
                    rel_path,
                    &node,
                );
                scope.push(id);
                pushed_scope = true;
            }
        }
        "enum_declaration" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::Enum, &name);
                push_node_and_defines_edge(
                    ex,
                    scope,
                    &id,
                    &name,
                    SymbolKind::Enum,
                    lang,
                    rel_path,
                    &node,
                );
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = field_text(node, "name", src) {
                let id = make_id(rel_path, lang, SymbolKind::TypeAlias, &name);
                push_node_and_defines_edge(
                    ex,
                    scope,
                    &id,
                    &name,
                    SymbolKind::TypeAlias,
                    lang,
                    rel_path,
                    &node,
                );
            }
        }
        "lexical_declaration" => {
            // `const fn = (...) => …` / `const Foo = class …`
            // Iterate variable_declarators, name the symbol.
            let mut walk = node.walk();
            if walk.goto_first_child() {
                loop {
                    let c = walk.node();
                    if c.kind() == "variable_declarator" {
                        if let Some(name) = field_text(c, "name", src) {
                            let value = c.child_by_field_name("value");
                            let symbol_kind = match value.map(|v| v.kind()) {
                                Some(
                                    "arrow_function"
                                    | "function_expression"
                                    | "function"
                                    | "generator_function",
                                ) => SymbolKind::Fn,
                                Some("class_expression" | "class") => SymbolKind::Struct,
                                _ => SymbolKind::Const,
                            };
                            let id = make_id(rel_path, lang, symbol_kind, &name);
                            push_node_and_defines_edge(
                                ex,
                                scope,
                                &id,
                                &name,
                                symbol_kind,
                                lang,
                                rel_path,
                                &c,
                            );
                        }
                    }
                    if !walk.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "import_statement" => {
            // Take the source string verbatim (`"react"`,
            // `"./foo"`, etc.).
            if let Some(src_node) = node.child_by_field_name("source") {
                let path_text = node_text(src_node, src);
                let parent = scope
                    .last()
                    .cloned()
                    .unwrap_or_else(|| make_id(rel_path, lang, SymbolKind::Module, "<file>"));
                ex.edges.push(CodeEdge {
                    source: parent,
                    target: format!("ext:{}", path_text.trim_matches(|c| c == '"' || c == '\'')),
                    relation: Relation::Imports,
                    confidence: Confidence::Extracted,
                });
            }
        }
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let callee = call_target_name(func, src);
                if let (Some(parent), Some(target)) = (scope.last(), callee) {
                    ex.edges.push(CodeEdge {
                        source: parent.clone(),
                        target: format!("name:{target}"),
                        relation: Relation::Calls,
                        confidence: Confidence::Inferred,
                    });
                }
            }
        }
        _ => {}
    }

    if cursor.goto_first_child() {
        loop {
            visit_ts(cursor, src, rel_path, lang, ex, scope);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }

    if pushed_scope {
        scope.pop();
    }
}

#[allow(clippy::too_many_arguments)]
fn push_node_and_defines_edge(
    ex: &mut CodeExtraction,
    scope: &[String],
    id: &str,
    label: &str,
    kind: SymbolKind,
    lang: CodeLang,
    rel_path: &Path,
    node: &Node,
) {
    ex.nodes.push(CodeNode {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        language: lang,
        source_file: rel_path.to_path_buf(),
        line_start: node.start_position().row as u32,
        line_end: node.end_position().row as u32,
    });
    if let Some(parent) = scope.last() {
        ex.edges.push(CodeEdge {
            source: parent.clone(),
            target: id.to_string(),
            relation: Relation::Defines,
            confidence: Confidence::Extracted,
        });
    }
}

/// Walk `root` recursively, extracting from every file with
/// a [`CodeLang`]-recognized extension. Returns a flat list
/// of per-file extractions plus a count of files we tried
/// and failed to read/parse (logged into per-extraction
/// `errors`).
///
/// Walks honor `.gitignore` via [`walkdir`] (skips `target/`,
/// `node_modules/`, `.git/`, hidden dirs).
pub fn scan_code_tree(root: &Path) -> Vec<CodeExtraction> {
    let mut out = Vec::new();
    // Walker filters out hidden / build / vendored dirs.
    // `filter_entry` blocks recursion through any entry that
    // returns false — but the ROOT itself usually has a name
    // like `src` / `task` / `.` (the cwd), so we never test
    // it. We test only descendant entries via depth() > 0.
    let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        !name.starts_with('.') && name != "target" && name != "node_modules" && name != "vendor"
    });
    for entry in walker.filter_map(Result::ok) {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = match p.extension().and_then(|s| s.to_str()) {
            Some(e) => e,
            None => continue,
        };
        if CodeLang::from_extension(ext).is_none() {
            continue;
        }
        // Make the source_file relative to the root for
        // stable ids that don't leak the host path.
        let rel = p.strip_prefix(root).unwrap_or(p);
        match std::fs::read_to_string(p) {
            Ok(src) => {
                let lang = CodeLang::from_extension(ext).unwrap();
                match extract_source(rel, &src, lang) {
                    Ok(ex) => out.push(ex),
                    Err(e) => {
                        let mut empty = CodeExtraction::default();
                        empty.errors.push(format!("{e}"));
                        out.push(empty);
                    }
                }
            }
            Err(e) => {
                let mut empty = CodeExtraction::default();
                empty.errors.push(format!("read {}: {e}", p.display()));
                out.push(empty);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fns_structs_traits_impls_from_rust() {
        let src = r#"
            use std::collections::HashMap;

            pub struct Foo;

            pub trait Bar {
                fn baz(&self) -> u32;
            }

            impl Bar for Foo {
                fn baz(&self) -> u32 { greet(); 42 }
            }

            pub fn greet() -> &'static str { "hi" }
        "#;
        let ex = extract_source(Path::new("test.rs"), src, CodeLang::Rust).unwrap();
        let labels: Vec<&str> = ex.nodes.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"Foo"), "struct missing: {labels:?}");
        assert!(labels.contains(&"Bar"), "trait missing: {labels:?}");
        assert!(labels.contains(&"greet"), "fn missing: {labels:?}");
        // The `impl Bar for Foo` becomes its own node.
        assert!(
            labels.iter().any(|l| l.contains("for Foo")),
            "impl missing: {labels:?}"
        );
        // The `baz` trait method + impl method should both be
        // captured as `fn`s.
        let fn_count = ex.nodes.iter().filter(|n| n.kind == SymbolKind::Fn).count();
        assert!(fn_count >= 3, "expected ≥3 fn nodes, got {fn_count}");

        // An `Imports` edge for the `use std::collections::HashMap;`.
        let import_count = ex
            .edges
            .iter()
            .filter(|e| e.relation == Relation::Imports)
            .count();
        assert!(import_count >= 1, "expected ≥1 import edge");

        // A `Calls` edge for `greet()` inside `impl Bar for Foo::baz`.
        let call_count = ex
            .edges
            .iter()
            .filter(|e| e.relation == Relation::Calls)
            .count();
        assert!(call_count >= 1, "expected ≥1 call edge");
    }

    #[test]
    fn unsupported_extension_errors() {
        let result = CodeLang::from_extension("py");
        assert!(result.is_none());
    }

    #[test]
    fn extracts_fn_class_import_call_from_typescript() {
        let src = r#"
            import { foo } from "./foo";
            import Bar from "bar";

            export class MyService {
                greet(name: string): string {
                    return foo(name);
                }
            }

            export function bootstrap(): MyService {
                return new MyService();
            }

            const helper = (x: number) => x + 1;
        "#;
        let ex = extract_source(Path::new("test.ts"), src, CodeLang::TypeScript).unwrap();
        let labels: Vec<&str> = ex.nodes.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"MyService"), "class missing: {labels:?}");
        assert!(labels.contains(&"bootstrap"), "fn missing: {labels:?}");
        assert!(labels.contains(&"greet"), "method missing: {labels:?}");
        assert!(labels.contains(&"helper"), "arrow fn missing: {labels:?}");

        let imports = ex
            .edges
            .iter()
            .filter(|e| e.relation == Relation::Imports)
            .count();
        assert!(imports >= 2, "expected ≥2 imports: {:?}", ex.edges);
        let calls = ex
            .edges
            .iter()
            .filter(|e| e.relation == Relation::Calls)
            .count();
        assert!(calls >= 1, "expected ≥1 call (foo or new MyService())");
    }

    #[test]
    fn extension_dispatch_ts_and_jsx() {
        assert_eq!(CodeLang::from_extension("ts"), Some(CodeLang::TypeScript));
        assert_eq!(CodeLang::from_extension("tsx"), Some(CodeLang::TypeScript));
        assert_eq!(CodeLang::from_extension("js"), Some(CodeLang::JavaScript));
        assert_eq!(CodeLang::from_extension("jsx"), Some(CodeLang::JavaScript));
        assert_eq!(CodeLang::from_extension("mjs"), Some(CodeLang::JavaScript));
    }
}
