//! Compiling an in-editor snapshot.
//!
//! The server runs the same front end as `based check` — discover a project's
//! `.bsl` set, overlay any unsaved editor buffers, parse + check — then keeps the
//! result (facts + diagnostics + a line index per file) so inlay-hint / hover /
//! diagnostic requests are served without recompiling. The `FileId` a span carries
//! is the index into `sources`, exactly as the CLI builds it.
//!
//! A workspace holds *many* projects: `.bsl` rides along inside a host repo, so the
//! opened folder is rarely the schema's `based.toml` dir (D5/D9). Each open file is
//! resolved to its owning project by walking up to the nearest `based.toml`
//! ([`find_manifest_root`]) and one snapshot is compiled per project — so cross-file
//! references inside a manifest resolve, and multiple embedded schemas in one
//! workspace stay independent. A file under no manifest keeps a single-file fallback.

use based_ast::{
    Assign, BaseType, Clause, Decl, Field, FileId, Ident, Member, Model, Mutation, NamedFilter,
    Param, ParamRef, Predicate, Primitive, Query, QueryBody, ScopeDecl, Shape, ShapeField,
    ShapeValue, Span, TypeExpr, Value, WriteStmt,
};
use based_diagnostics::Diagnostic;
use based_facts::{Fact, FactKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, InlayHint, InlayHintKind, InlayHintLabel,
    InlayHintLabelPart, InlayHintTooltip, Location, Position, Range, SymbolKind, Url,
};

/// A compiled view of the project the server answers requests from.
pub struct Snapshot {
    /// Sources indexed by `FileId` — `sources[i]` is the file spans stamp `FileId(i)`.
    pub sources: Vec<(PathBuf, String)>,
    /// Byte-offset <-> LSP position index, parallel to `sources`.
    pub lines: Vec<LineIndex>,
    pub facts: Vec<Fact>,
    /// The parsed declarations (spans stamped with each file's `FileId`), retained
    /// so position-based requests (go-to-definition) can resolve references against
    /// the same AST the front end checked. Empty if no file parsed clean.
    pub decls: Vec<Decl>,
    /// Diagnostics carrying a span (attachable to a file). Spanless project-level
    /// diagnostics are surfaced separately (as window messages).
    pub diagnostics: Vec<Diagnostic>,
    /// Project-level diagnostics with no span (e.g. a malformed manifest).
    pub project_diagnostics: Vec<Diagnostic>,
}

impl Snapshot {
    /// `FileId` index for a path, matched by canonicalized path.
    pub fn file_id_of(&self, path: &Path) -> Option<usize> {
        let want = canon(path);
        self.sources.iter().position(|(p, _)| canon(p) == want)
    }

    /// Resolve a model/type reference under the cursor to the span of the matching
    /// declaration's name. `(fid, offset)` is the byte offset within file `fid`.
    /// Returns the name span of the `Model` (or `Shape`) the reference names — a
    /// `Location` the editor jumps to — or `None` if the cursor is not on a type
    /// reference, or the referenced type is undeclared in this project.
    pub fn definition_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &&Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        // A model/shape type reference → that decl's name.
        if let Some(target) = collect_type_refs(&self.decls).into_iter().find(under) {
            return self.decls.iter().find_map(|d| match d {
                Decl::Model(m) if m.name.node == target.node => Some(m.name.span),
                Decl::Shape(s) if s.name.node == target.node => Some(s.name.span),
                _ => None,
            });
        }
        // A `@scope Name` / `scoped Name` reference → the `scope Name (…)` decl's name.
        if let Some(target) = collect_scope_refs(&self.decls).into_iter().find(under) {
            return self.decls.iter().find_map(|d| match d {
                Decl::Scope(s) if s.name.node == target.node => Some(s.name.span),
                _ => None,
            });
        }
        // A field-reference path segment (`placed_by`, `placed_by.name`, a `where`/
        // `order`/write-assign column) → the field it names, walked through relations
        // from the statically-known root.
        if let Some(f) = self.field_ref_at(fid, offset) {
            return Some(f.name.span);
        }
        // A declaration's own name resolves to itself. Conventional for go-to-def on a
        // definition, and load-bearing for the inverse inlay: VS Code activates a label
        // part's `location` by running go-to-def *at* it (LSP 3.17), and that location
        // is the forward edge's declaration — so it must resolve, or the click is inert.
        self.decl_name_at(fid, offset)
    }

    /// The name span of a declaration whose own name the cursor sits on: a model, one
    /// of its fields, a shape, a query/mutation/filter, or a scope. `None` elsewhere.
    fn decl_name_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    if under(&m.name) {
                        return Some(m.name.span);
                    }
                    for mem in &m.members {
                        if let Member::Field(f) = mem {
                            if under(&f.name) {
                                return Some(f.name.span);
                            }
                        }
                    }
                }
                Decl::Shape(s) if under(&s.name) => return Some(s.name.span),
                Decl::Query(q) if under(&q.name) => return Some(q.name.span),
                Decl::Mutation(m) if under(&m.name) => return Some(m.name.span),
                Decl::Filter(f) if under(&f.name) => return Some(f.name.span),
                Decl::Scope(s) if under(&s.name) => return Some(s.name.span),
                _ => {}
            }
        }
        None
    }

    /// The inlay hints for file `fid`: one per derived fact anchored in it, each at
    /// the end of its line. The inferred-inverse hint is a command-clickable label
    /// part linking to the forward edge it pairs through (`OrderItem.order`); the
    /// model/callable-wide facts are plain `tag label` strings; a scope is *written*,
    /// not derived, so it carries no inlay (hover only). The caller filters by the
    /// requested viewport range.
    pub fn inlay_hints(&self, fid: usize) -> Vec<InlayHint> {
        let idx = &self.lines[fid];
        let mut hints = Vec::new();
        for f in &self.facts {
            if f.span.file.0 as usize != fid {
                continue;
            }
            let position = match f.kind {
                FactKind::InferredInverse
                | FactKind::InferredIndex
                | FactKind::CtxRequirement
                | FactKind::ResolvedQuery => idx.end_of_line(f.span.start as usize),
                FactKind::Scope => continue,
            };
            // An inferred inverse links to the forward edge it pairs through, so the
            // `via Model.field` hint is command-clickable; other facts are plain text.
            let label = match (f.kind, f.nav) {
                (FactKind::InferredInverse, Some(nav)) => {
                    InlayHintLabel::LabelParts(vec![InlayHintLabelPart {
                        value: f.label.clone(),
                        location: self.nav_location(nav),
                        tooltip: None,
                        command: None,
                    }])
                }
                _ => InlayHintLabel::String(format!("{} {}", f.kind.tag(), f.label)),
            };
            hints.push(InlayHint {
                position,
                label,
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: Some(InlayHintTooltip::String(f.detail.clone())),
                padding_left: Some(true),
                padding_right: None,
                data: None,
            });
        }
        hints
    }

    /// A cross-file `Location` for a span, resolving its `FileId` to the owning URI —
    /// the command-click target of an inlay label part (an inverse's forward edge).
    fn nav_location(&self, span: Span) -> Option<Location> {
        let fid = span.file.0 as usize;
        let (path, _) = self.sources.get(fid)?;
        let uri = Url::from_file_path(path).ok()?;
        Some(Location {
            uri,
            range: span_range(span, &self.lines[fid]),
        })
    }

    /// Rich "what" for the symbol under the cursor — a field's `name: Type`, or a
    /// model/shape/scope/callable signature — for hover (rust-analyzer's baseline).
    /// `None` when the cursor is not on a resolvable symbol.
    pub fn hover_at(&self, fid: usize, offset: u32) -> Option<String> {
        let under = |id: &&Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        // A field reference (path segment) → the field's signature.
        if let Some(f) = self.field_ref_at(fid, offset) {
            return Some(field_hover(f));
        }
        // A model/shape type reference → the referenced decl's signature.
        if let Some(t) = collect_type_refs(&self.decls).into_iter().find(under) {
            if let Some(h) = self.decl_hover_by_name(&t.node) {
                return Some(h);
            }
        }
        // A `@scope`/`scoped` reference → the scope contract's shape.
        if let Some(t) = collect_scope_refs(&self.decls).into_iter().find(under) {
            if let Some(h) = self.scope_hover(&t.node) {
                return Some(h);
            }
        }
        // Otherwise the cursor may sit on a declaration's own name.
        self.decl_site_hover(fid, offset)
    }

    /// The field a reference path segment under the cursor names. Every path in a
    /// shape body / query clause / mutation write is rooted at a statically-known
    /// model (the shape's `from`, the statement target, the write model) and walked
    /// segment-by-segment through relation edges; the segment the cursor is on
    /// resolves to its declaring field. `None` off any such segment.
    fn field_ref_at(&self, fid: usize, offset: u32) -> Option<&Field> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for (root, segs) in self.field_paths() {
            if let Some(i) = segs.iter().position(under) {
                return self.walk_path(root, &segs[..=i]);
            }
        }
        None
    }

    /// Resolve a path prefix against `root`, returning the field its last segment
    /// names. Intermediate segments must be relation edges (they advance the model);
    /// the final segment may be a scalar or relation. `None` if any segment is not a
    /// field of the model reached so far.
    fn walk_path(&self, root: &str, segs: &[Ident]) -> Option<&Field> {
        let mut model = self.model_by_name(root)?;
        let mut last: Option<&Field> = None;
        for seg in segs {
            let field = model.members.iter().find_map(|m| match m {
                Member::Field(f) if f.name.node == seg.node => Some(f),
                _ => None,
            })?;
            last = Some(field);
            if let BaseType::Model(t) = &field.ty.base {
                match self.model_by_name(&t.node) {
                    Some(m) => model = m,
                    None => break,
                }
            }
        }
        last
    }

    /// Every field-reference path in the project, each paired with the model it is
    /// rooted at. Covers shape bodies, query `where`/`order` clauses, and mutation
    /// write `where`/assign columns — the contexts whose root model is statically
    /// known. (Filters are omitted: their root is the polymorphic call site.)
    fn field_paths(&self) -> Vec<(&str, &[Ident])> {
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                Decl::Shape(s) => self.shape_paths(s.from.node.as_str(), &s.body, &mut out),
                Decl::Query(q) => match &q.body {
                    // A block's clauses root at the explicit statement target.
                    QueryBody::Block(stmt) => {
                        for c in &stmt.clauses {
                            clause_paths(c, stmt.model.node.as_str(), &mut out);
                        }
                    }
                    // Inline clauses root at the query's inferred target (its return).
                    QueryBody::Inline(clauses) => {
                        if let Some(root) = self.query_root(q) {
                            for c in clauses {
                                clause_paths(c, root, &mut out);
                            }
                        }
                    }
                    QueryBody::Bare => {}
                },
                Decl::Mutation(m) => write_paths(&m.body, &mut out),
                _ => {}
            }
        }
        out
    }

    /// Collect a shape body's field paths (rooted at `from`), recursing into `field {
    /// … }` sub-objects against the relation's target model.
    fn shape_paths<'a>(
        &'a self,
        from: &'a str,
        body: &'a [ShapeField],
        out: &mut Vec<(&'a str, &'a [Ident])>,
    ) {
        for sf in body {
            match sf {
                ShapeField::Bare(id) => out.push((from, std::slice::from_ref(id))),
                ShapeField::Rename {
                    value: ShapeValue::Path(p),
                    ..
                } => out.push((from, &p.segments)),
                ShapeField::Rename { .. } => {} // raw-SQL value: no field path
                ShapeField::Nest { field, body } => {
                    out.push((from, std::slice::from_ref(field)));
                    if let Some(target) = self.relation_target(from, &field.node) {
                        self.shape_paths(target, body, out);
                    }
                }
            }
        }
    }

    /// The model a relation `field` on `model` points at, if it is a relation edge.
    fn relation_target(&self, model: &str, field: &str) -> Option<&str> {
        let m = self.model_by_name(model)?;
        m.members.iter().find_map(|mem| match mem {
            Member::Field(f) if f.name.node == field => match &f.ty.base {
                BaseType::Model(t) => Some(t.node.as_str()),
                _ => None,
            },
            _ => None,
        })
    }

    /// The model an inline/bare query reads from: its return shape's `from`, or the
    /// return model itself when the return type is a bare model.
    fn query_root(&self, q: &Query) -> Option<&str> {
        let ret = q.ret.ty.node.as_str();
        self.decls.iter().find_map(|d| match d {
            Decl::Shape(s) if s.name.node == ret => Some(s.from.node.as_str()),
            Decl::Model(m) if m.name.node == ret => Some(m.name.node.as_str()),
            _ => None,
        })
    }

    /// A model or shape decl's one-line hover, by name.
    fn decl_hover_by_name(&self, name: &str) -> Option<String> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == name => Some(model_hover(m)),
            Decl::Shape(s) if s.name.node == name => Some(shape_hover(s)),
            _ => None,
        })
    }

    /// A scope decl's one-line hover (`scope Name (col: Type = $ctx.field, …)`).
    fn scope_hover(&self, name: &str) -> Option<String> {
        self.decls.iter().find_map(|d| match d {
            Decl::Scope(s) if s.name.node == name => Some(scope_hover(s)),
            _ => None,
        })
    }

    /// Hover for a declaration's own name (the cursor sits on the thing being
    /// declared, not a reference to it): the model/field/shape/callable/scope it
    /// introduces.
    fn decl_site_hover(&self, fid: usize, offset: u32) -> Option<String> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    if under(&m.name) {
                        return Some(model_hover(m));
                    }
                    for mem in &m.members {
                        if let Member::Field(f) = mem {
                            if under(&f.name) {
                                return Some(field_hover(f));
                            }
                        }
                    }
                }
                Decl::Shape(s) if under(&s.name) => return Some(shape_hover(s)),
                Decl::Query(q) if under(&q.name) => return Some(query_hover(q)),
                Decl::Mutation(m) if under(&m.name) => return Some(mutation_hover(m)),
                Decl::Filter(f) if under(&f.name) => return Some(filter_hover(f)),
                Decl::Scope(s) if under(&s.name) => return Some(scope_hover(s)),
                _ => {}
            }
        }
        None
    }

    /// Document symbols for file `fid` — the outline the editor shows (breadcrumbs
    /// / ⇧⌘O). Models nest their fields as `Field` children; queries, mutations,
    /// shapes, and filters are flat top-level symbols. Each symbol's `range` is its
    /// declaration extent and its `selection_range` the name (both required to nest,
    /// LSP contains the latter in the former). Only decls declared in `fid` appear.
    pub fn document_symbols(&self, fid: usize) -> Vec<DocumentSymbol> {
        let idx = &self.lines[fid];
        let here = |span: Span| span.file.0 as usize == fid;
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                // Model → Struct; its fields (not indexes / soft-overrides) → Field children.
                Decl::Model(m) if here(m.span) => {
                    let children = m
                        .members
                        .iter()
                        .filter_map(|mem| match mem {
                            Member::Field(f) => {
                                Some(symbol(&f.name, SymbolKind::FIELD, f.span, idx, None))
                            }
                            _ => None,
                        })
                        .collect();
                    out.push(symbol(
                        &m.name,
                        SymbolKind::STRUCT,
                        m.span,
                        idx,
                        Some(children),
                    ));
                }
                Decl::Shape(s) if here(s.span) => {
                    out.push(symbol(&s.name, SymbolKind::INTERFACE, s.span, idx, None))
                }
                Decl::Query(q) if here(q.span) => {
                    out.push(symbol(&q.name, SymbolKind::FUNCTION, q.span, idx, None))
                }
                Decl::Mutation(m) if here(m.span) => {
                    out.push(symbol(&m.name, SymbolKind::METHOD, m.span, idx, None))
                }
                Decl::Filter(f) if here(f.span) => {
                    out.push(symbol(&f.name, SymbolKind::FUNCTION, f.span, idx, None))
                }
                _ => {}
            }
        }
        out
    }

    /// Context-aware completions at `(fid, offset)`. The buffer under an edit is
    /// often unparseable, so instead of parsing the half-typed text we read the
    /// source *prefix* before the cursor and classify by its trigger character (the
    /// token immediately before the partial word being typed).
    ///
    /// - `@` → the decorator set.
    /// - `<ident>.` → the fields of the base model, but only when the base is
    ///   statically resolvable (a path rooted in a shape's `from` model or a query
    ///   block's target, walked through relation fields); else nothing — precision
    ///   over recall.
    /// - `:` → primitives + model names (a field's type annotation).
    /// - `->` → model + shape names (a return type).
    /// - anything else → the keyword/function set + model names (the vocabulary
    ///   bucket; works even when the file does not yet parse).
    pub fn completions(&self, fid: usize, offset: u32) -> Vec<CompletionItem> {
        let src = &self.sources[fid].1;
        let before = &src[..(offset as usize).min(src.len())];
        // Drop the partial word under the cursor, then any spaces, to expose the
        // trigger character that classifies the context.
        let head = before
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
            .trim_end();
        match head.chars().next_back() {
            Some('@') => decorator_items(),
            Some('.') => self.field_items_after_dot(fid, offset, head),
            Some(':') => self.type_items(),
            Some('>') if head.ends_with("->") => self.return_type_items(),
            _ => self.default_items(),
        }
    }

    /// Fields of the model a dotted path resolves to, when the base is statically
    /// known. The path is rooted at the enclosing shape's `from` or query block's
    /// target and walked segment-by-segment through relation fields; any segment
    /// that is not a relation (or an unknown root) yields nothing.
    fn field_items_after_dot(&self, fid: usize, offset: u32, head: &str) -> Vec<CompletionItem> {
        let segs = trailing_path(head);
        if segs.is_empty() {
            return Vec::new();
        }
        let Some(root) = self.root_model_at(fid, offset) else {
            return Vec::new();
        };
        let Some(mut model) = self.model_by_name(root) else {
            return Vec::new();
        };
        for seg in &segs {
            let Some(field) = model.members.iter().find_map(|m| match m {
                Member::Field(f) if &f.name.node == seg => Some(f),
                _ => None,
            }) else {
                return Vec::new();
            };
            let BaseType::Model(target) = &field.ty.base else {
                return Vec::new();
            };
            match self.model_by_name(&target.node) {
                Some(m) => model = m,
                None => return Vec::new(),
            }
        }
        model
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Field(f) => Some(item(&f.name.node, CompletionItemKind::FIELD)),
                _ => None,
            })
            .collect()
    }

    /// The model a dotted path is rooted at, if the cursor sits in a decl whose
    /// root is cheaply known: a shape (its `from`) or a query block (its target).
    fn root_model_at(&self, fid: usize, offset: u32) -> Option<&str> {
        let here = |s: Span| s.file.0 as usize == fid && s.start <= offset && offset < s.end;
        self.decls.iter().find_map(|d| match d {
            Decl::Shape(s) if here(s.span) => Some(s.from.node.as_str()),
            Decl::Query(q) if here(q.span) => match &q.body {
                QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                _ => None,
            },
            _ => None,
        })
    }

    fn model_by_name(&self, name: &str) -> Option<&Model> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == name => Some(m),
            _ => None,
        })
    }

    /// Field type annotation position: the primitives plus every model name.
    fn type_items(&self) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = PRIMITIVES
            .iter()
            .map(|p| item(p, CompletionItemKind::KEYWORD))
            .collect();
        items.extend(self.model_name_items());
        items
    }

    /// Return type position: models and shapes (a callable may return either).
    fn return_type_items(&self) -> Vec<CompletionItem> {
        let mut items = self.model_name_items();
        items.extend(self.decls.iter().filter_map(|d| match d {
            Decl::Shape(s) => Some(item(&s.name.node, CompletionItemKind::INTERFACE)),
            _ => None,
        }));
        items
    }

    /// The fallback vocabulary: keywords, functions, and model names.
    fn default_items(&self) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = KEYWORDS
            .iter()
            .map(|k| item(k, CompletionItemKind::KEYWORD))
            .collect();
        items.extend(
            based_sema::KNOWN_FUNCS
                .iter()
                .map(|f| item(f, CompletionItemKind::FUNCTION)),
        );
        items.extend(self.model_name_items());
        items
    }

    fn model_name_items(&self) -> Vec<CompletionItem> {
        self.decls
            .iter()
            .filter_map(|d| match d {
                Decl::Model(m) => Some(item(&m.name.node, CompletionItemKind::STRUCT)),
                _ => None,
            })
            .collect()
    }
}

/// The DSL keyword vocabulary, derived from the parser's positionally-recognized
/// keywords (there is no `model` keyword — a model is a bare `UpperName { … }`).
const KEYWORDS: &[&str] = &[
    "shape",
    "query",
    "mutation",
    "filter",
    "from",
    "guard",
    "unscoped",
    "get",
    "list",
    "create",
    "update",
    "delete",
    "restore",
    "tx",
    "hard",
    "where",
    "order",
    "page",
    "unindexed",
    "index",
    "unique",
    "default",
    "column",
    "asc",
    "desc",
    "offset",
    "with",
    "count",
    "and",
    "or",
    "not",
    "in",
    "has",
    "on",
    "full",
    "sql",
    "read",
    "max_rows",
    "unsafe",
];

/// Primitive type spellings (the `Primitive` variants), offered in type position.
const PRIMITIVES: &[&str] = &[
    "text",
    "int",
    "bool",
    "timestamp",
    "date",
    "json",
    "uuid",
    "Id",
];

/// Everything an author writes after `@`: the engine-understood model decorators
/// (`based_sema::KNOWN_DECORATORS`) plus the member-level `@index` and the
/// `@was("old")` rename directive.
fn decorator_items() -> Vec<CompletionItem> {
    based_sema::KNOWN_DECORATORS
        .iter()
        .copied()
        .chain(["index", "was"])
        .map(|d| item(d, CompletionItemKind::PROPERTY))
        .collect()
}

/// The dotted identifier chain immediately before a `.` in `head` (which ends with
/// that `.`) — the base path a field completion resolves. `["a", "b"]` for `a.b.`;
/// empty when the char before the `.` is not an identifier (e.g. `^.`, `$ctx` aside).
fn trailing_path(head: &str) -> Vec<String> {
    let Some(mut rest) = head.strip_suffix('.') else {
        return Vec::new();
    };
    let mut segs: Vec<String> = Vec::new();
    loop {
        let n: usize = rest
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum();
        if n == 0 {
            break;
        }
        segs.push(rest[rest.len() - n..].to_string());
        rest = &rest[..rest.len() - n];
        match rest.strip_suffix('.') {
            Some(r) => rest = r,
            None => break,
        }
    }
    segs.reverse();
    segs
}

/// One completion item with just a label + kind (no snippet / fuzzy detail).
fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        ..Default::default()
    }
}

/// Build one `DocumentSymbol` from a declaration's name + extent span.
#[allow(deprecated)] // `deprecated` is a required struct field, set to None.
fn symbol(
    name: &Ident,
    kind: SymbolKind,
    extent: Span,
    idx: &LineIndex,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.node.clone(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: span_range(extent, idx),
        selection_range: span_range(name.span, idx),
        children,
    }
}

/// A source `Span`'s byte range as an LSP `Range` via the owning file's index.
fn span_range(span: Span, idx: &LineIndex) -> Range {
    Range::new(
        idx.position(span.start as usize),
        idx.position(span.end as usize),
    )
}

/// Every model/type-reference identifier across the AST, with its span — the sites
/// a name *points at* a declared model or shape (not the declarations themselves).
/// Traverses all reference-bearing positions: field types, opt-in inverses, shape
/// `from`, query/mutation return types + param types + `get`/`list` targets, write
/// targets (incl. nested `tx`), and filter param types.
fn collect_type_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Model(m) => {
                for member in &m.members {
                    if let Member::Field(f) = member {
                        collect_type_expr(&f.ty, &mut out);
                        if let Some(inv) = &f.inverse {
                            out.push(&inv.model);
                        }
                    }
                }
            }
            Decl::Shape(s) => out.push(&s.from),
            Decl::Scope(s) => {
                for t in &s.terms {
                    collect_type_expr(&t.ty, &mut out);
                }
            }
            Decl::Query(q) => {
                out.push(&q.ret.ty);
                for p in &q.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
                if let QueryBody::Block(stmt) = &q.body {
                    out.push(&stmt.model);
                }
            }
            Decl::Mutation(m) => {
                out.push(&m.ret.ty);
                for p in &m.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
                collect_write_targets(&m.body, &mut out);
            }
            Decl::Filter(f) => {
                for p in &f.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// Every scope-name reference identifier across the AST, with its span — the sites a name
/// *points at* a `scope` decl: `@scope Name[, …]` on a model, `scoped Name[, …]` on a
/// query or mutation. Used for go-to-definition into the `scope` decl.
fn collect_scope_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Model(m) => {
                for r in &m.scopes {
                    out.extend(r.names.iter());
                }
            }
            Decl::Query(q) => {
                if let Some(s) = &q.scoped {
                    out.extend(s.names.iter());
                }
            }
            Decl::Mutation(m) => {
                if let Some(s) = &m.scoped {
                    out.extend(s.names.iter());
                }
            }
            _ => {}
        }
    }
    out
}

/// The model reference in a type expression, if its base is a model (not a primitive).
fn collect_type_expr<'a>(ty: &'a TypeExpr, out: &mut Vec<&'a Ident>) {
    if let BaseType::Model(id) = &ty.base {
        out.push(id);
    }
}

/// Write-target models, recursing through `tx` blocks; `raw` carries no target.
fn collect_write_targets<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a Ident>) {
    for w in body {
        match w {
            WriteStmt::Create { model, .. }
            | WriteStmt::Update { model, .. }
            | WriteStmt::Delete { model, .. }
            | WriteStmt::Restore { model, .. }
            | WriteStmt::HardDelete { model, .. } => out.push(model),
            WriteStmt::Tx(inner) => collect_write_targets(inner, out),
            WriteStmt::Raw(_) => {}
        }
    }
}

// ---- Field-reference path collectors (root model + its column paths) --------
// Each pushes `(root_model, segments)` so a cursor on any segment resolves through
// the relation walk in `Snapshot::walk_path`.

/// A query clause's field paths, rooted at `root`: a `where` predicate's columns and
/// an `order` clause's sort paths (a `page` clause carries none).
fn clause_paths<'a>(c: &'a Clause, root: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match c {
        Clause::Where(p) => pred_paths(p, root, out),
        Clause::Order(terms) => {
            for t in terms {
                out.push((root, &t.path.segments));
            }
        }
        Clause::Page(_) | Clause::Unindexed(_) => {}
    }
}

/// A predicate's field paths (both sides of a comparison, bare bool columns, filter-
/// call value paths), all rooted at `root`. Filter *names* are not fields.
fn pred_paths<'a>(p: &'a Predicate, root: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_paths(a, root, out);
            pred_paths(b, root, out);
        }
        Predicate::Not(inner) => pred_paths(inner, root, out),
        Predicate::Cmp { path, value, .. } => {
            out.push((root, &path.segments));
            value_paths(value, root, out);
        }
        Predicate::Bare(path) => out.push((root, &path.segments)),
        Predicate::FilterCall { args, .. } => {
            for v in args {
                value_paths(v, root, out);
            }
        }
        Predicate::Raw(_) => {}
    }
}

/// A value's field path, when it is one (a column reference or a function argument
/// that is itself a column); params, literals, and `^.field` back-refs carry none.
fn value_paths<'a>(v: &'a Value, root: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match v {
        Value::Path(p) => out.push((root, &p.segments)),
        Value::Func(fc) => {
            for a in &fc.args {
                value_paths(a, root, out);
            }
        }
        _ => {}
    }
}

/// A mutation write body's field paths, rooted at each statement's write model,
/// recursing through `tx`. Covers `where` predicates and assign columns/values.
fn write_paths<'a>(body: &'a [WriteStmt], out: &mut Vec<(&'a str, &'a [Ident])>) {
    for w in body {
        match w {
            WriteStmt::Create { model, assigns } => assign_paths(assigns, model.node.as_str(), out),
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => {
                pred_paths(where_, model.node.as_str(), out);
                assign_paths(assigns, model.node.as_str(), out);
            }
            WriteStmt::Delete { model, where_ }
            | WriteStmt::Restore { model, where_ }
            | WriteStmt::HardDelete { model, where_ } => {
                pred_paths(where_, model.node.as_str(), out)
            }
            WriteStmt::Tx(inner) => write_paths(inner, out),
            WriteStmt::Raw(_) => {}
        }
    }
}

/// A create/update's assign paths: the target column and any column-valued RHS.
fn assign_paths<'a>(assigns: &'a [Assign], model: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    for a in assigns {
        out.push((model, std::slice::from_ref(&a.col)));
        value_paths(&a.value, model, out);
    }
}

// ---- Hover renderers ("what", rust-analyzer baseline) -----------------------

/// A `TypeExpr` as source writes it: base spelling + `?` (optional) + `[]` (many).
fn type_str(ty: &TypeExpr) -> String {
    let mut s = match &ty.base {
        BaseType::Primitive(p) => primitive_str(*p).to_string(),
        BaseType::Model(id) => id.node.clone(),
    };
    if ty.optional {
        s.push('?');
    }
    if ty.many {
        s.push_str("[]");
    }
    s
}

/// Primitive → its DSL spelling (`Id` keeps its casing, the rest lowercase).
fn primitive_str(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "Id",
    }
}

/// `$ctx.org` and the like, from a `ParamRef` (`$` + name + dotted path).
fn paramref_str(pr: &ParamRef) -> String {
    let mut s = format!("${}", pr.name.node);
    for seg in &pr.path {
        s.push('.');
        s.push_str(&seg.node);
    }
    s
}

/// A field's hover: its `name: Type` signature, plus a cardinality note for relations.
fn field_hover(f: &Field) -> String {
    let sig = format!("{}: {}", f.name.node, type_str(&f.ty));
    match &f.ty.base {
        BaseType::Model(m) => {
            let card = if f.ty.many { "to-many" } else { "to-one" };
            format!("```based\n{sig}\n```\n{card} relation to `{}`", m.node)
        }
        BaseType::Primitive(_) => format!("```based\n{sig}\n```"),
    }
}

/// A model's hover: `model Name` and its declared-field count.
fn model_hover(m: &Model) -> String {
    let n = m
        .members
        .iter()
        .filter(|mem| matches!(mem, Member::Field(_)))
        .count();
    let plural = if n == 1 { "" } else { "s" };
    format!("```based\nmodel {}\n```\n{n} field{plural}", m.name.node)
}

/// A shape's hover: `shape Name from Model`.
fn shape_hover(s: &Shape) -> String {
    format!("```based\nshape {} from {}\n```", s.name.node, s.from.node)
}

/// A scope's hover: `scope Name (col: Type = $ctx.field, …)`.
fn scope_hover(s: &ScopeDecl) -> String {
    let terms = s
        .terms
        .iter()
        .map(|t| {
            format!(
                "{}: {} = {}",
                t.col.node,
                type_str(&t.ty),
                paramref_str(&t.ctx)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("```based\nscope {} ({terms})\n```", s.name.node)
}

/// A query's hover: `query name(params) -> Ret[]`.
fn query_hover(q: &Query) -> String {
    let card = if q.ret.many { "[]" } else { "" };
    format!(
        "```based\nquery {}({}) -> {}{card}\n```",
        q.name.node,
        params_str(&q.params),
        q.ret.ty.node,
    )
}

/// A mutation's hover: `mutation name(params) -> Ret[]`.
fn mutation_hover(m: &Mutation) -> String {
    let card = if m.ret.many { "[]" } else { "" };
    format!(
        "```based\nmutation {}({}) -> {}{card}\n```",
        m.name.node,
        params_str(&m.params),
        m.ret.ty.node,
    )
}

/// A filter's hover: `filter name(params)`.
fn filter_hover(f: &NamedFilter) -> String {
    format!(
        "```based\nfilter {}({})\n```",
        f.name.node,
        params_str(&f.params)
    )
}

/// A parameter list rendered `name: Type` (type dropped when the param is untyped).
fn params_str(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| match &p.ty {
            Some(t) => format!("{}: {}", p.name.node, type_str(t)),
            None => p.name.node.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Walk up `file`'s ancestor directories to the nearest one holding a `based.toml`,
/// returning that directory — the manifest root that *owns* the file (the
/// rust-analyzer / tsserver project-marker model). `None` when no ancestor has a
/// manifest, i.e. the file rides under no project (single-file fallback).
pub fn find_manifest_root(file: &Path) -> Option<PathBuf> {
    // Canonicalize so the walk is over real ancestors; a not-yet-saved buffer
    // falls back to its raw path, whose parents are still meaningful.
    let start = canon(file);
    let mut dir = start.parent();
    while let Some(d) = dir {
        if d.join(based_manifest::MANIFEST_NAME).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Compile the manifest project rooted at `root` (the dir holding `based.toml`),
/// with `overlays` (canonical path -> unsaved buffer text) taking precedence over
/// on-disk contents. Overlays for files outside this project are simply ignored.
pub fn compile_manifest(root: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    match based_manifest::discover(root) {
        Ok(project) => {
            let paths = project.files.into_iter().map(|f| f.path).collect();
            compile_paths(paths, overlays, Vec::new())
        }
        // Manifest present but unreadable/malformed: surface it as a project-level
        // diagnostic and still compile this project's open buffers so the editor
        // keeps giving single-file feedback.
        Err(diags) => {
            let mut ps: Vec<PathBuf> = overlays
                .keys()
                .filter(|p| find_manifest_root(p).as_deref() == Some(root))
                .cloned()
                .collect();
            ps.sort();
            compile_paths(ps, overlays, diags)
        }
    }
}

/// Compile a single `.bsl` file under no manifest in isolation (the fallback for a
/// file that belongs to no project — cross-file references cannot resolve here).
pub fn compile_loose(file: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    compile_paths(vec![file.to_path_buf()], overlays, Vec::new())
}

/// Read + parse + check a fixed file set, preferring open buffers over disk, into a
/// snapshot. `project_diagnostics` carries any spanless project-level issues.
fn compile_paths(
    paths: Vec<PathBuf>,
    overlays: &HashMap<PathBuf, String>,
    project_diagnostics: Vec<Diagnostic>,
) -> Snapshot {
    // Read every file, preferring an open buffer over disk.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
    for path in paths {
        let text = overlays
            .get(&canon(&path))
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
        sources.push((path, text));
    }

    // Parse each file; collect decls only if every file parsed clean (sema assumes
    // well-formed input, matching the CLI's precondition).
    let mut decls: Vec<Decl> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_ok = true;
    for (i, (_, src)) in sources.iter().enumerate() {
        match based_parser::parse_file(src, FileId(i as u32)) {
            Ok(sf) => decls.extend(sf.decls),
            Err(diags) => {
                diagnostics.extend(diags);
                parse_ok = false;
            }
        }
    }

    let mut facts = Vec::new();
    if parse_ok {
        let (schema, diags) = based_sema::check(&decls);
        diagnostics.extend(diags);
        facts = based_facts::facts(&schema, &decls);
    }

    let lines = sources.iter().map(|(_, s)| LineIndex::new(s)).collect();
    Snapshot {
        sources,
        lines,
        facts,
        decls,
        diagnostics,
        project_diagnostics,
    }
}

/// Canonicalize for path comparison; fall back to the raw path if the file does
/// not resolve (e.g. an unsaved buffer whose path may not exist on disk yet).
pub fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Byte-offset <-> LSP `Position` mapping for one file. LSP positions are 0-based
/// `(line, character)` where `character` counts UTF-16 code units (the protocol
/// default); we compute that faithfully so multibyte source lines map correctly.
pub struct LineIndex {
    src: String,
    /// Byte offset of each line's first byte.
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex {
            src: src.to_string(),
            line_starts,
        }
    }

    /// Byte offset -> `(line, utf16-character)`.
    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.src.len());
        // Last line whose start is <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line];
        let character = self.src[start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position::new(line as u32, character as u32)
    }

    /// `(line, utf16-character)` -> byte offset.
    pub fn offset(&self, pos: Position) -> usize {
        let line = pos.line as usize;
        let Some(&start) = self.line_starts.get(line) else {
            return self.src.len();
        };
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.src.len());
        let mut units = 0usize;
        for (i, c) in self.src[start..end].char_indices() {
            if units >= pos.character as usize {
                return start + i;
            }
            units += char::len_utf16(c);
        }
        end
    }

    /// Position at the end of the line containing `offset` (trailing newline
    /// excluded) — where a per-declaration inlay hint reads best.
    pub fn end_of_line(&self, offset: usize) -> Position {
        let offset = offset.min(self.src.len());
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line];
        let end = self.src[start..]
            .find('\n')
            .map(|n| start + n)
            .unwrap_or(self.src.len());
        self.position(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn position_offset_round_trip_ascii() {
        let src = "Order {\n  name: text\n}\n";
        let idx = LineIndex::new(src);
        // "name" starts at line 1 (0-based), char 2.
        let at = src.find("name").unwrap();
        assert_eq!(idx.position(at), Position::new(1, 2));
        assert_eq!(idx.offset(Position::new(1, 2)), at);
    }

    #[test]
    fn position_counts_utf16_code_units() {
        // "é" is one UTF-16 unit but two UTF-8 bytes; "𐐷" is two UTF-16 units.
        let src = "// é𐐷 x\n";
        let idx = LineIndex::new(src);
        let x = src.find('x').unwrap();
        // chars before x on the line: '/', '/', ' ', 'é'(1), '𐐷'(2), ' ' = 7 units.
        assert_eq!(idx.position(x), Position::new(0, 7));
        assert_eq!(idx.offset(Position::new(0, 7)), x);
    }

    #[test]
    fn end_of_line_skips_the_newline() {
        let src = "Order {\n  x: int\n}\n";
        let idx = LineIndex::new(src);
        let brace = src.find('{').unwrap();
        // End of line 0 = after "Order {" (7 chars), before the '\n'.
        assert_eq!(idx.end_of_line(brace), Position::new(0, 7));
    }

    #[test]
    fn compile_commerce_has_facts_and_no_diagnostics() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        assert!(snap.project_diagnostics.is_empty());
        assert!(!snap.sources.is_empty());
        // The inferred inverse on `Order.items` is surfaced.
        assert!(
            snap.facts
                .iter()
                .any(|f| f.label.contains("via OrderItem.order")),
            "{:?}",
            snap.facts
        );
    }

    /// A file embedded in a host repo resolves to its own schema's `based.toml`,
    /// not the opened workspace root — this is the C3 fix's core.
    #[test]
    fn find_manifest_root_walks_up_to_nearest_manifest() {
        let ws = TempWorkspace::new("walkup");
        ws.write("based.toml", "");
        ws.write("order/model.bsl", "Order { name: text }\n");
        let file = ws.path("order/model.bsl");

        let root = find_manifest_root(&file).expect("manifest above the file");
        assert_eq!(root, canon(&ws.root));

        // A file with no manifest anywhere above it has no project.
        let orphan = TempWorkspace::new("orphan");
        orphan.write("loose.bsl", "Order { name: text }\n");
        assert_eq!(find_manifest_root(&orphan.path("loose.bsl")), None);
    }

    /// Opening the repo root (no `based.toml` there) and editing a file whose model
    /// references a *sibling* file must resolve the whole manifest project — no
    /// spurious `E0110`, unlike the single-file fallback.
    #[test]
    fn two_manifest_workspace_resolves_each_project_independently() {
        let ws = TempWorkspace::new("two_manifest");
        // Project A: a two-file schema with a cross-file relation.
        ws.write("a/based.toml", "");
        ws.write("a/org.bsl", "Org { name: text }\n");
        ws.write("a/user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        // Project B: an independent, unrelated schema.
        ws.write("b/based.toml", "");
        ws.write("b/widget.bsl", "Widget { label: text }\n");

        // Each project compiles clean on its own manifest.
        let a = compile_manifest(&ws.path("a"), &HashMap::new());
        assert!(a.diagnostics.is_empty(), "project A: {:?}", a.diagnostics);
        assert!(a.project_diagnostics.is_empty());
        let b = compile_manifest(&ws.path("b"), &HashMap::new());
        assert!(b.diagnostics.is_empty(), "project B: {:?}", b.diagnostics);

        // The two projects are disjoint: B never sees A's models.
        assert!(b.sources.iter().all(|(p, _)| !p.ends_with("user.bsl")));

        // The cross-file reference (`User.org -> Org`) is what the manifest scope
        // buys: compiling `user.bsl` in isolation cannot see `Org` -> E0110.
        let loose = compile_loose(&ws.path("a/user.bsl"), &HashMap::new());
        assert!(
            loose.diagnostics.iter().any(|d| d.code == "E0110"),
            "single-file fallback should not resolve the sibling model: {:?}",
            loose.diagnostics
        );
    }

    /// A cursor inside a model *reference* (`org: Org`) resolves to that model's
    /// declaration span in the sibling file that declares it — the go-to-definition
    /// core, hermetic over a two-file manifest project.
    #[test]
    fn goto_definition_resolves_model_reference_cross_file() {
        let ws = TempWorkspace::new("gotodef");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Cursor mid-`Org` in the `org: Org` field type of user.bsl.
        let user_fid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let src = &snap.sources[user_fid].1;
        let off = (src.find("Org").unwrap() + 1) as u32;
        let def = snap
            .definition_at(user_fid, off)
            .expect("reference resolves to a declaration");

        // It points at the `Org` model's name span, in org.bsl.
        let (def_path, def_src) = &snap.sources[def.file.0 as usize];
        assert!(def_path.ends_with("org.bsl"), "{def_path:?}");
        assert_eq!(&def_src[def.start as usize..def.end as usize], "Org");

        // Whitespace (a non-reference offset) resolves to nothing.
        let ws_off = src.find("\n  name").unwrap() as u32;
        assert_eq!(snap.definition_at(user_fid, ws_off), None);
    }

    /// Go-to-definition from a `@scope Name` (model) or `scoped Name` (callable)
    /// reference resolves to the `scope Name (…)` decl's name span — the both-sides
    /// scope contract is navigable from either reference.
    #[test]
    fn goto_definition_resolves_scope_reference() {
        let ws = TempWorkspace::new("gotoscope");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "scope Tenant (org: Org = $ctx.org)\n\
             Org { name: text }\n\
             @scope Tenant\n\
             Widget { org: Org  name: text }\n\
             query widgets() -> Widget[] scoped Tenant { list Widget; }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            !snap
                .diagnostics
                .iter()
                .any(|d| d.severity == based_diagnostics::Severity::Error),
            "{:?}",
            snap.diagnostics
        );

        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // The `scope Tenant` decl's own name span (the definition target).
        let decl_at = src.find("scope Tenant").unwrap() + "scope ".len();
        let def_span = src[decl_at..decl_at + "Tenant".len()].to_string();
        assert_eq!(def_span, "Tenant");

        // From the `@scope Tenant` reference on the model.
        let deco_ref = (src.find("@scope Tenant").unwrap() + "@scope ".len() + 1) as u32;
        let d1 = snap.definition_at(fid, deco_ref).expect("@scope resolves");
        assert_eq!(&src[d1.start as usize..d1.end as usize], "Tenant");
        assert_eq!(d1.start as usize, decl_at);

        // From the `scoped Tenant` reference on the query.
        let scoped_ref = (src.find("scoped Tenant").unwrap() + "scoped ".len() + 1) as u32;
        let d2 = snap
            .definition_at(fid, scoped_ref)
            .expect("scoped resolves");
        assert_eq!(&src[d2.start as usize..d2.end as usize], "Tenant");
        assert_eq!(d2.start as usize, decl_at);
    }

    /// The inferred-inverse inlay is a *clickable* label part: `via OrderItem.order`
    /// whose location points at the `order` forward edge in order_item/model.bsl, so
    /// command-clicking the hint navigates to the edge it pairs through.
    #[test]
    fn inverse_inlay_is_a_clickable_label_part() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();

        let hints = snap.inlay_hints(model_fid);
        // Find the inverse hint (the only label-parts hint) and inspect its part.
        let parts = hints
            .iter()
            .find_map(|h| match &h.label {
                InlayHintLabel::LabelParts(p) => Some(p),
                _ => None,
            })
            .expect("an inverse hint rendered as label parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].value, "via OrderItem.order");

        // The part carries a location, and it points at `order` in order_item/model.bsl.
        let loc = parts[0].location.as_ref().expect("clickable location");
        assert!(
            loc.uri.path().ends_with("order_item/model.bsl"),
            "{}",
            loc.uri
        );
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let oi_src = &snap.sources[oi_fid].1;
        let off = snap.lines[oi_fid].offset(loc.range.start);
        assert!(oi_src[off..].starts_with("order"), "lands on the edge name");

        // VS Code activates the label part by running go-to-def *at* its location
        // (LSP 3.17), so that must resolve — otherwise the click is inert even though
        // the link underlines. The forward edge's declaration resolves to itself.
        let def = snap
            .definition_at(oi_fid, off as u32)
            .expect("the click's go-to-def-at-location round-trips");
        assert!(oi_src[def.start as usize..].starts_with("order"));
    }

    /// Go-to-definition on a *field-reference* path resolves each segment to the
    /// field it names, walking through relations from the shape's `from` root — even
    /// when the walk crosses into another file's model (`placed_by.name` → `User.name`).
    #[test]
    fn goto_definition_resolves_field_reference_path() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // `buyer = placed_by.name` in OrderCard (order/model.bsl).
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[model_fid].1;
        let base = src.find("placed_by.name").unwrap();

        // Cursor on `placed_by` → the `Order.placed_by` field decl, same file.
        let def = snap
            .definition_at(model_fid, (base + 1) as u32)
            .expect("placed_by resolves");
        let (dp, ds) = &snap.sources[def.file.0 as usize];
        assert!(dp.ends_with("order/model.bsl"), "{dp:?}");
        assert_eq!(&ds[def.start as usize..def.end as usize], "placed_by");

        // Cursor on the trailing `.name` → the `User.name` field, in user/model.bsl.
        let name_off = base + "placed_by.".len() + 1;
        let ndef = snap
            .definition_at(model_fid, name_off as u32)
            .expect("name resolves through the relation");
        let (np, nsrc) = &snap.sources[ndef.file.0 as usize];
        assert!(np.ends_with("user/model.bsl"), "{np:?}");
        assert_eq!(&nsrc[ndef.start as usize..ndef.end as usize], "name");

        // A bare shape field (`status`) resolves to the local column, too.
        let st = src.find("\n  status\n").map(|p| p + 3).unwrap();
        let sdef = snap.definition_at(model_fid, st as u32).unwrap();
        assert_eq!(
            &src[sdef.start as usize..sdef.end as usize],
            "status",
            "bare shape field resolves to its column"
        );
    }

    /// Hover gives a rust-analyzer-style "what" for the symbol under the cursor: a
    /// field's `name: Type` (+ relation note), and model/shape signatures — for both
    /// references and the declarations themselves.
    #[test]
    fn hover_reports_field_and_decl_signatures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // The `placed_by: User` field declaration → its signature + relation note.
        let decl = src.find("placed_by:").unwrap() + 1;
        let h = snap.hover_at(fid, decl as u32).expect("field decl hover");
        assert!(h.contains("placed_by: User"), "{h}");
        assert!(h.contains("to-one relation to `User`"), "{h}");

        // A field *reference* in the shape (`placed_by.name`) → the walked field.
        let refoff = src.find("placed_by.name").unwrap() + "placed_by.".len() + 1;
        let hr = snap.hover_at(fid, refoff as u32).expect("field ref hover");
        assert!(hr.contains("name: text"), "{hr}");

        // A model type reference (`items: OrderItem[]`) → `model OrderItem`.
        let mref = src.find("OrderItem[]").unwrap() + 1;
        let hm = snap.hover_at(fid, mref as u32).expect("model ref hover");
        assert!(hm.contains("model OrderItem"), "{hm}");

        // The shape's own name → `shape OrderCard from Order`.
        let sh = src.find("OrderCard from Order").unwrap() + 1;
        let hs = snap.hover_at(fid, sh as u32).expect("shape decl hover");
        assert!(hs.contains("shape OrderCard from Order"), "{hs}");
    }

    /// Field-reference go-to-def reaches beyond shapes: a query block's `where`/`order`
    /// columns and a mutation's create-assign columns all resolve to the model field,
    /// rooted at the statement target / write model.
    #[test]
    fn goto_definition_resolves_query_and_mutation_columns() {
        let ws = TempWorkspace::new("colrefs");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { label: text  qty: int? }\n\
             shape W from Widget { label }\n\
             query find() -> W[] { list Widget where (qty > 0) order (label asc); }\n\
             mutation add(l: text) -> W { create Widget { label = $l }; }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            !snap
                .diagnostics
                .iter()
                .any(|d| d.severity == based_diagnostics::Severity::Error),
            "{:?}",
            snap.diagnostics
        );
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let label_decl = src.find("label: text").unwrap();

        // `where (qty > 0)` → the `qty` column.
        let qty = src.find("qty > 0").unwrap() + 1;
        let d = snap.definition_at(fid, qty as u32).expect("where column");
        assert_eq!(&src[d.start as usize..d.end as usize], "qty");

        // `order (label asc)` → the `label` column (its declaration).
        let ord = src.find("label asc").unwrap() + 1;
        let d = snap.definition_at(fid, ord as u32).expect("order column");
        assert_eq!(d.start as usize, label_decl);

        // `create Widget { label = $l }` → the `label` column.
        let asg = src.find("label = $l").unwrap() + 1;
        let d = snap.definition_at(fid, asg as u32).expect("assign column");
        assert_eq!(d.start as usize, label_decl);
    }

    /// Document symbols for a file expose its models (with fields nested), shapes,
    /// queries, and mutations with the right kinds — asserted over the real commerce
    /// schema so nesting + kind mapping are proven end to end.
    #[test]
    fn document_symbols_over_commerce_order_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // order/model.bsl: the `Order` model (Struct) with its fields nested, plus
        // the `OrderCard` shape (Interface) — both flat top-level symbols.
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let syms = snap.document_symbols(model_fid);
        let order = syms
            .iter()
            .find(|s| s.name == "Order")
            .expect("Order symbol");
        assert_eq!(order.kind, SymbolKind::STRUCT);
        let fields = order.children.as_ref().expect("Order has field children");
        assert!(fields.iter().all(|f| f.kind == SymbolKind::FIELD));
        for want in ["org", "placed_by", "status", "total", "items"] {
            assert!(fields.iter().any(|f| f.name == want), "field {want}");
        }
        // The name selection range sits inside the declaration extent.
        assert!(order.selection_range.start >= order.range.start);
        assert!(order.selection_range.end <= order.range.end);
        let card = syms
            .iter()
            .find(|s| s.name == "OrderCard")
            .expect("OrderCard shape");
        assert_eq!(card.kind, SymbolKind::INTERFACE);

        // order/queries.bsl: queries → Function, the mutation → Method.
        let q_fid = snap.file_id_of(&root.join("order/queries.bsl")).unwrap();
        let qsyms = snap.document_symbols(q_fid);
        let q = qsyms
            .iter()
            .find(|s| s.name == "my_org_orders")
            .expect("query symbol");
        assert_eq!(q.kind, SymbolKind::FUNCTION);
        let m = qsyms
            .iter()
            .find(|s| s.name == "place_order")
            .expect("mutation symbol");
        assert_eq!(m.kind, SymbolKind::METHOD);
        // Symbols are file-scoped: no cross-file leakage into the query file.
        assert!(qsyms.iter().all(|s| s.name != "Order"));
    }

    /// Completion is context-aware off the source prefix: model names in a type
    /// annotation, a base model's fields after a resolvable `.`, the decorator set
    /// after `@`, and the keyword/model vocabulary otherwise. Hermetic over a small
    /// manifest project so the resolution (root model + relation walk) is proven.
    #[test]
    fn completions_by_context() {
        let ws = TempWorkspace::new("completion");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        ws.write(
            "card.bsl",
            "shape UserCard from User {\n  city = org.name\n}\n",
        );
        ws.write("dec.bsl", "@table(\"things\")\nThing { label: text }\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        let labels = |items: &[CompletionItem]| -> Vec<String> {
            items.iter().map(|c| c.label.clone()).collect()
        };

        // Type position: after `org:` in user.bsl → model names + primitives.
        let ufid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let at = snap.sources[ufid].1.find("org:").unwrap() + "org:".len();
        let ty = snap.completions(ufid, at as u32);
        assert!(ty
            .iter()
            .any(|c| c.label == "Org" && c.kind == Some(CompletionItemKind::STRUCT)));
        assert!(labels(&ty).contains(&"text".to_string()));

        // Field after a resolvable `.`: `org.` in the shape (root User, org → Org)
        // completes Org's fields, and *only* fields (precision over recall).
        let cfid = snap.file_id_of(&ws.path("card.bsl")).unwrap();
        let dot = snap.sources[cfid].1.find("org.").unwrap() + "org.".len();
        let fields = snap.completions(cfid, dot as u32);
        assert!(fields
            .iter()
            .any(|c| c.label == "name" && c.kind == Some(CompletionItemKind::FIELD)));
        assert!(fields
            .iter()
            .all(|c| c.kind == Some(CompletionItemKind::FIELD)));

        // A non-resolvable base yields nothing, not wrong suggestions: an overlay
        // whose path steps through a scalar (`name.`) has no model to walk into.
        let mut ov = HashMap::new();
        ov.insert(
            canon(&ws.path("card.bsl")),
            "shape UserCard from User {\n  x = name.\n}\n".to_string(),
        );
        let osnap = compile_manifest(&ws.root, &ov);
        let ofid = osnap.file_id_of(&ws.path("card.bsl")).unwrap();
        let sdot = osnap.sources[ofid].1.find("name.").unwrap() + "name.".len();
        assert!(osnap.completions(ofid, sdot as u32).is_empty());

        // Decorator set after `@`.
        let dfid = snap.file_id_of(&ws.path("dec.bsl")).unwrap();
        let atpos = snap.sources[dfid].1.find("@table").unwrap() + 1;
        let decos = snap.completions(dfid, atpos as u32);
        for want in ["soft_delete", "scope", "table", "index", "was"] {
            assert!(
                labels(&decos).contains(&want.to_string()),
                "decorator {want}"
            );
        }
        assert!(decos
            .iter()
            .all(|c| c.kind == Some(CompletionItemKind::PROPERTY)));

        // Vocabulary bucket at a blank position: keywords + model names.
        let kw = snap.completions(ufid, 0);
        assert!(kw
            .iter()
            .any(|c| c.label == "query" && c.kind == Some(CompletionItemKind::KEYWORD)));
        assert!(labels(&kw).contains(&"User".to_string()));
        assert!(labels(&kw).contains(&"Thing".to_string()));
    }

    /// A throwaway workspace dir under the system temp, removed on drop.
    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("based-lsp-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            TempWorkspace { root }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }

        fn write(&self, rel: &str, contents: &str) {
            let p = self.path(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
