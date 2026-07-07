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

use based_ast::{BaseType, Decl, FileId, Ident, Member, QueryBody, Span, TypeExpr, WriteStmt};
use based_diagnostics::Diagnostic;
use based_facts::Fact;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};

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
        let target = collect_type_refs(&self.decls).into_iter().find(|id| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        })?;
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == target.node => Some(m.name.span),
            Decl::Shape(s) if s.name.node == target.node => Some(s.name.span),
            _ => None,
        })
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
            snap.facts.iter().any(|f| f.label.contains("via order")),
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
