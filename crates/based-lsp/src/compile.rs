//! Compiling an in-editor snapshot.
//!
//! The server runs the same front end as `based check` — discover a project's
//! `.bsl` set, overlay any unsaved editor buffers, parse + check — then keeps the
//! result (facts + diagnostics + a line index per file) so inlay-hint / hover /
//! diagnostic requests are served without recompiling. The `FileId` a span carries
//! is the index into `sources`, exactly as the CLI builds it.
//!
//! A workspace holds many projects: each open file resolves to its owning project by
//! walking up to the nearest `based.toml` ([`find_manifest_root`]), and one snapshot
//! is compiled per project — so cross-file references inside a manifest resolve and
//! embedded schemas stay independent. A file under no manifest gets a single-file
//! fallback.

use based_ast::{
    Assign, BaseType, Clause, Decl, EnumDecl, Field, FileId, Ident, Member, Model, Modifier,
    Mutation, NamedFilter, Op, Param, ParamBinding, ParamRef, Predicate, Primitive, Query,
    QueryBody, RawPart, RawSql, ScopeDecl, Shape, ShapeField, ShapeValue, Span, TypeExpr, Value,
    VariantValue, WriteStmt,
};
use based_diagnostics::Diagnostic;
use based_facts::{Fact, FactKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, FoldingRange, FoldingRangeKind, InlayHint,
    InlayHintKind, InlayHintLabel, InlayHintLabelPart, InlayHintTooltip, Location, Position, Range,
    SelectionRange, SymbolInformation, SymbolKind, TextEdit, Url,
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
    /// The resolved schema, when every file parsed clean. Read by rename to map a
    /// field/model to its physical column/table for the data-preserving `@was` edit.
    pub schema: Option<based_sema::CheckedSchema>,
    /// The manifest root, when this snapshot is a manifest project — the dir whose
    /// `migrations/` a `@was`-preserving rename consults.
    pub migrations_root: Option<PathBuf>,
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
        if let Some(id) = collect_type_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.type_ref_target(id) {
                return Some(s);
            }
        }
        // A `@scope Name` / `scoped Name` reference → the `scope Name (…)` decl's name.
        if let Some(id) = collect_scope_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.scope_ref_target(id) {
                return Some(s);
            }
        }
        // A `filter(...)` call → the `filter Name(…)` decl's name.
        if let Some(id) = collect_filter_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.filter_ref_target(id) {
                return Some(s);
            }
        }
        // An enum variant in value/default position (`where status = paid`, `default
        // pending`) → the variant's declaration inside its `enum`. Tried before field
        // references so a variant is never misread as a same-named column.
        if let Some(s) = self.variant_ref_at(fid, offset) {
            return Some(s);
        }
        // A field-reference path segment (`placed_by`, `placed_by.name`, a `where`/
        // `order`/write-assign column) → the field it names, walked through relations
        // from the statically-known root.
        if let Some(f) = self.field_ref_at(fid, offset) {
            return Some(f.name.span);
        }
        // A callable param (`buyer: Id` decl or a `$buyer` use in its body) → the param
        // decl's name. Params are callable-local, so the target span scopes the rename.
        if let Some(s) = self.param_ref_at(fid, offset) {
            return Some(s);
        }
        // A `$ctx.<field>` bag field (a callable use or a `scope … = $ctx.field` term) →
        // the field's canonical occurrence. The `$ctx` bag is coherent by name across
        // the schema, so one field renames everywhere it is used.
        if let Some(s) = self.ctx_ref_at(fid, offset) {
            return Some(s);
        }
        // A declaration's own name resolves to itself. Conventional for go-to-def on a
        // definition, and load-bearing for the inverse inlay: VS Code activates a label
        // part's `location` by running go-to-def *at* it (LSP 3.17), and that location
        // is the forward edge's declaration — so it must resolve, or the click is inert.
        self.decl_name_at(fid, offset)
    }

    /// The declaration-name span a model/shape/enum type reference names, if declared here.
    fn type_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == id.node => Some(m.name.span),
            Decl::Shape(s) if s.name.node == id.node => Some(s.name.span),
            Decl::Enum(e) if e.name.node == id.node => Some(e.name.span),
            _ => None,
        })
    }

    /// The `scope` decl-name span a `@scope`/`scoped` reference names.
    fn scope_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Scope(s) if s.name.node == id.node => Some(s.name.span),
            _ => None,
        })
    }

    /// The `filter` decl-name span a `filter(...)` call names.
    fn filter_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Filter(f) if f.name.node == id.node => Some(f.name.span),
            _ => None,
        })
    }

    /// Every reference site that resolves to the same declaration as the symbol under
    /// the cursor — the inverse of `definition_at`. Powers find-references and (later)
    /// rename. Covers model/shape type references, `@scope`/`scoped` references, filter
    /// calls, field-reference path segments (walked through relations), and the inverse
    /// back-edges that pair through a forward field (so a forward edge's references
    /// include the `Model[]` inverse that joins through it — the "back-follow"). With
    /// `include_decl`, the declaration's own name is included. Deduped, span-ordered.
    pub fn references_at(&self, fid: usize, offset: u32, include_decl: bool) -> Vec<Span> {
        let Some(target) = self.definition_at(fid, offset) else {
            return Vec::new();
        };
        let mut out = Vec::new();

        // Model / shape type references naming the same declaration.
        for id in collect_type_refs(&self.decls) {
            if self.type_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Scope references (`@scope` / `scoped`).
        for id in collect_scope_refs(&self.decls) {
            if self.scope_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Filter calls.
        for id in collect_filter_refs(&self.decls) {
            if self.filter_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Enum variant uses: when the target is a variant declaration, every value/
        // default-position use of that variant. Enum-local — a same-named variant in a
        // different enum is keyed by its own enum, so it is left untouched.
        if let Some((enum_name, variant)) = self.variant_of_decl_span(target) {
            for (seg, en) in self.variant_use_sites() {
                if en == enum_name && seg.node == variant {
                    out.push(seg.span);
                }
            }
        }
        // Field-reference path segments resolving to the target field.
        for (root, segs) in self.field_paths() {
            for (i, seg) in segs.iter().enumerate() {
                if self.walk_path(root, &segs[..=i]).map(|f| f.name.span) == Some(target) {
                    out.push(seg.span);
                }
            }
        }
        // Inverse back-edges: an inferred inverse's `nav` is its paired forward edge,
        // so a forward field's references include the `Model[]` inverse joining through
        // it. (Explicit `(Model.field)` inverses are picked up as field refs below.)
        for f in &self.facts {
            if f.nav == Some(target) {
                out.push(f.span);
            }
        }
        // Explicit inverse pairings `(Model.field)`: the `field` part references it.
        for id in collect_explicit_inverse_fields(&self.decls) {
            if self.explicit_inverse_target(id.0, id.1) == Some(target) {
                out.push(id.1.span);
            }
        }
        // Callable param uses: every `$param` in the callable owning the target param.
        for d in &self.decls {
            if let Some(p) = decl_params(d).iter().find(|p| p.name.span == target) {
                for pr in callable_param_refs(d) {
                    if pr.path.is_empty() && pr.name.node == p.name.node {
                        out.push(pr.name.span);
                    }
                }
            }
        }
        // `$ctx.<field>` uses: every occurrence of the bag field whose canonical is the
        // target (the scope-term binding and every callable use share one name).
        let ctx = self.ctx_occurrences();
        if let Some(name) = ctx
            .iter()
            .map(|(n, _)| n)
            .find(|n| self.ctx_canonical_span(n) == Some(target))
            .cloned()
        {
            for (n, span) in &ctx {
                if *n == name {
                    out.push(*span);
                }
            }
        }

        if include_decl {
            out.push(target);
        }
        out.sort_by_key(|s| (s.file.0, s.start, s.end));
        out.dedup();
        out
    }

    /// The workspace edit renaming the symbol under the cursor to `new_name`, grouped
    /// by owning file. One text edit per occurrence that *textually* spells the old
    /// name — so the inverse back-edge (a differently-named field that merely pairs
    /// through the symbol, e.g. `Order.items` for `OrderItem.order`) is left untouched,
    /// unlike find-references which lists it. `None` when the cursor is not on a
    /// renameable symbol or `new_name` is not a valid identifier.
    pub fn rename_edits(
        &self,
        fid: usize,
        offset: u32,
        new_name: &str,
    ) -> Option<HashMap<Url, Vec<TextEdit>>> {
        if !is_ident(new_name) {
            return None;
        }
        let target = self.definition_at(fid, offset)?;
        let old = self.span_text(target)?.to_string();
        let mut edits: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        for span in self.references_at(fid, offset, true) {
            // Rewrite only sites literally spelling the old name; a back-edge pairs
            // through the symbol under a different name and must not be renamed.
            if self.span_text(span) != Some(old.as_str()) {
                continue;
            }
            let f = span.file.0 as usize;
            let Ok(uri) = Url::from_file_path(&self.sources[f].0) else {
                continue;
            };
            edits.entry(uri).or_default().push(TextEdit {
                range: span_range(span, &self.lines[f]),
                new_text: new_name.to_string(),
            });
        }
        // If the renamed symbol maps to a live DB column/table, also insert a `@was`
        // so the generated migration preserves data (rename, not drop+add).
        if let Some((uri, edit)) = self.was_edit_for_rename(target, &old, new_name) {
            edits.entry(uri).or_default().push(edit);
        }
        (!edits.is_empty()).then_some(edits)
    }

    /// The identifier range under the cursor to offer for rename (prepareRename), or
    /// `None` when the cursor is not on a renameable symbol. The range is the extent of
    /// the identifier the cursor sits in; renameability is gated on the same resolver
    /// go-to-def uses, so keywords, literals, and whitespace decline.
    pub fn prepare_rename_range(&self, fid: usize, offset: u32) -> Option<Range> {
        self.definition_at(fid, offset)?;
        let (start, end) = word_extent(&self.sources[fid].1, offset)?;
        let idx = &self.lines[fid];
        Some(Range::new(idx.position(start), idx.position(end)))
    }

    /// Folding ranges for file `fid`: one region per top-level declaration whose body
    /// spans more than one line — a model, shape, scope, query, mutation, or filter.
    /// The range runs from the body's opening `{` (so the `Name {` header stays
    /// visible when collapsed) down to the closing delimiter; a brace-less decl (a
    /// multi-line scope / inline query / filter) folds from its first line. Extents
    /// come straight off the parsed decl spans — no text structure is re-derived.
    /// The file's canonical formatting, or `None` when there is nothing to apply —
    /// it doesn't parse (unformattable) or is already formatted. Delegates to the same
    /// `based_fmt` the CLI's `based fmt` runs, so the editor and CLI never diverge.
    pub fn format_document(&self, fid: usize) -> Option<String> {
        let src = &self.sources[fid].1;
        let formatted = based_fmt::format_source(src).ok()?;
        (formatted != *src).then_some(formatted)
    }

    pub fn folding_ranges(&self, fid: usize) -> Vec<FoldingRange> {
        let idx = &self.lines[fid];
        let src = &self.sources[fid].1;
        let mut out = Vec::new();
        for d in &self.decls {
            let span = decl_span(d);
            if span.file.0 as usize != fid {
                continue;
            }
            let (lo, hi) = (span.start as usize, (span.end as usize).min(src.len()));
            // Fold from the block's opening brace when there is one, else the decl's
            // own start; the close is the span's last byte (the `}` / `)` / `;`).
            let open = src[lo..hi].find('{').map(|i| lo + i).unwrap_or(lo);
            let start_line = idx.position(open).line;
            let end_line = idx.position(hi.saturating_sub(1)).line;
            if end_line > start_line {
                out.push(FoldingRange {
                    start_line,
                    end_line,
                    kind: Some(FoldingRangeKind::Region),
                    ..Default::default()
                });
            }
        }
        out
    }

    /// The selection-range hierarchy at `offset`: the nested ranges an editor cycles
    /// through on expand/shrink-selection, innermost first. Levels are the identifier
    /// token under the cursor, then each enclosing AST span that covers it (a model
    /// field's name → type → whole field, its declaration, and finally the whole
    /// file), each linked to its parent. `None` when the offset sits in no declaration
    /// and on no word (the caller falls back to a bare cursor range).
    pub fn selection_range(&self, fid: usize, offset: u32) -> Option<SelectionRange> {
        let src = &self.sources[fid].1;
        let contains =
            |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;

        // Candidate ranges covering the offset, from the AST spans plus the word token
        // and the whole file. Order is imposed below by width, so collection order is
        // free.
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        if let Some((s, e)) = word_extent(src, offset) {
            ranges.push((s as u32, e as u32));
        }
        for d in &self.decls {
            let dspan = decl_span(d);
            if !contains(dspan) {
                continue;
            }
            if let Decl::Model(m) = d {
                for mem in &m.members {
                    if let Member::Field(f) = mem {
                        if contains(f.span) {
                            for sp in [f.name.span, f.ty.span, f.span] {
                                if contains(sp) {
                                    ranges.push((sp.start, sp.end));
                                }
                            }
                        }
                    }
                }
            }
            ranges.push((dspan.start, dspan.end));
        }
        ranges.push((0, src.len() as u32));

        // Keep a strictly-nesting chain, narrowest first: sort by width, drop
        // duplicates and any range that doesn't contain the one kept before it.
        ranges.sort_by_key(|(s, e)| e - s);
        ranges.dedup();
        let mut chain: Vec<(u32, u32)> = Vec::new();
        for (s, e) in ranges {
            match chain.last() {
                Some(&(ps, pe)) if s <= ps && e >= pe && (s, e) != (ps, pe) => chain.push((s, e)),
                None => chain.push((s, e)),
                _ => {}
            }
        }

        // Build parent links from the widest inward; the returned root is innermost.
        let idx = &self.lines[fid];
        let mut node: Option<Box<SelectionRange>> = None;
        for (s, e) in chain.into_iter().rev() {
            let range = Range::new(idx.position(s as usize), idx.position(e as usize));
            node = Some(Box::new(SelectionRange {
                range,
                parent: node,
            }));
        }
        node.map(|b| *b)
    }

    /// The source text a span covers, within its owning file.
    fn span_text(&self, span: Span) -> Option<&str> {
        let (_, src) = self.sources.get(span.file.0 as usize)?;
        src.get(span.start as usize..span.end as usize)
    }

    /// The param-decl name span a cursor resolves to — whether it sits on a callable's
    /// `buyer: Id` param declaration or on a `$buyer` use in that callable's body.
    /// `None` off any param. Params are callable-local, so the returned span identifies
    /// exactly one callable's param.
    fn param_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            let params = decl_params(d);
            if params.is_empty() {
                continue;
            }
            if let Some(p) = params.iter().find(|p| under(&p.name)) {
                return Some(p.name.span);
            }
            for pr in callable_param_refs(d) {
                if pr.path.is_empty() && under(&pr.name) {
                    if let Some(p) = params.iter().find(|p| p.name.node == pr.name.node) {
                        return Some(p.name.span);
                    }
                }
            }
        }
        None
    }

    /// The canonical occurrence span of the `$ctx` bag field under the cursor, or
    /// `None` off any `$ctx.<field>`. A `$ctx` field is keyed by name (the bag is
    /// coherent across the schema), so every occurrence of one field maps to the same
    /// canonical (its first occurrence in file/offset order).
    fn ctx_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        for (name, span) in self.ctx_occurrences() {
            if span.file.0 as usize == fid && span.start <= offset && offset < span.end {
                return self.ctx_canonical_span(&name);
            }
        }
        None
    }

    /// Every `$ctx.<field>` occurrence in the project as `(field_name, segment_span)` —
    /// the field segment of each `scope … = $ctx.field` binding and each callable-body
    /// use. The span is the `field` segment (not the `$ctx` prefix), so a rename
    /// rewrites only the bag field name.
    fn ctx_occurrences(&self) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        let mut push = |pr: &ParamRef| {
            if pr.name.node == "ctx" {
                if let Some(seg) = pr.path.first() {
                    out.push((seg.node.clone(), seg.span));
                }
            }
        };
        for d in &self.decls {
            if let Decl::Scope(s) = d {
                for t in &s.terms {
                    push(&t.ctx);
                }
            }
            for pr in callable_param_refs(d) {
                push(pr);
            }
        }
        out
    }

    /// The first-in-order occurrence span of `$ctx.<name>` — the rename target every
    /// occurrence of that bag field resolves to.
    fn ctx_canonical_span(&self, name: &str) -> Option<Span> {
        self.ctx_occurrences()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, s)| s)
            .min_by_key(|s| (s.file.0, s.start, s.end))
    }

    /// The extra edit that makes a field/model rename **data-preserving**: a `@was("old")`
    /// naming the declaration's current physical column/table, so the next generated
    /// migration renames it (keeping data) instead of drop+add. Emitted only when the
    /// rename actually changes the physical name (no `(column …)` / `@table` override
    /// decouples it), the declaration has no `@was` already (an existing one still names
    /// the snapshot's column — a rename chain must keep the original), and the physical
    /// name is a **live** column/table in the project's latest captured snapshot (an
    /// uncaptured column needs no rename step). `None` outside those conditions.
    fn was_edit_for_rename(
        &self,
        target: Span,
        old: &str,
        new_name: &str,
    ) -> Option<(Url, TextEdit)> {
        if old == new_name {
            return None;
        }
        let schema = self.schema.as_ref()?;
        let prev = latest_snapshot(self.migrations_root.as_deref()?)?;
        for d in &self.decls {
            let Decl::Model(m) = d else { continue };
            // Model rename → `@was("old_table")` as a leading decorator line.
            if m.name.span == target {
                if m.decorators.iter().any(|dc| dc.name.node == "was")
                    || m.decorators.iter().any(|dc| dc.name.node == "table")
                {
                    return None;
                }
                let table = &schema.model(&m.name.node)?.table;
                prev.table(table)?;
                let fid = m.name.span.file.0 as usize;
                let at = line_start(&self.sources[fid].1, m.name.span.start);
                return self.insertion(fid, at, format!("@was(\"{table}\")\n"));
            }
            // Field rename → ` @was("old_col")` appended to the field's modifiers.
            for mem in &m.members {
                let Member::Field(f) = mem else { continue };
                if f.name.span != target {
                    continue;
                }
                if f.was.is_some()
                    || f.modifiers
                        .iter()
                        .any(|md| matches!(md, Modifier::Column(_)))
                {
                    return None;
                }
                let rmodel = schema.model(&m.name.node)?;
                let rmem = rmodel.member(&f.name.node)?;
                if matches!(rmem.kind, based_sema::MemberKind::Inverse { .. }) {
                    return None;
                }
                let col = rmem.physical_col().to_string();
                prev.table(&rmodel.table)?.column(&col)?;
                let fid = f.span.file.0 as usize;
                return self.insertion(fid, f.span.end as usize, format!(" @was(\"{col}\")"));
            }
        }
        None
    }

    /// A zero-width `TextEdit` inserting `text` at byte `offset` in file `fid`.
    fn insertion(&self, fid: usize, offset: usize, text: String) -> Option<(Url, TextEdit)> {
        let uri = Url::from_file_path(&self.sources[fid].0).ok()?;
        let pos = self.lines[fid].position(offset);
        Some((
            uri,
            TextEdit {
                range: Range::new(pos, pos),
                new_text: text,
            },
        ))
    }

    /// Resolve an explicit inverse's `(Model.field)` to that field's name span.
    fn explicit_inverse_target(&self, model: &Ident, field: &Ident) -> Option<Span> {
        let m = self.model_by_name(&model.node)?;
        m.members.iter().find_map(|mem| match mem {
            Member::Field(f) if f.name.node == field.node => Some(f.name.span),
            _ => None,
        })
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
                Decl::Enum(e) if under(&e.name) => return Some(e.name.span),
                _ => {}
            }
        }
        None
    }

    /// The inlay hints for file `fid`: one per derived fact anchored in it, each at
    /// the end of its line. The inferred-inverse hint is a command-clickable label
    /// part linking to the forward edge it pairs through (`OrderItem.order`); the
    /// index and resolved-query facts are plain `tag label` strings. The scope and
    /// `$ctx` contracts surface on hover, so they carry no inlay. The caller filters
    /// by the requested viewport range.
    pub fn inlay_hints(&self, fid: usize) -> Vec<InlayHint> {
        let idx = &self.lines[fid];
        let mut hints = Vec::new();
        for f in &self.facts {
            if f.span.file.0 as usize != fid {
                continue;
            }
            let position = match f.kind {
                FactKind::InferredInverse | FactKind::InferredIndex | FactKind::ResolvedQuery => {
                    idx.end_of_line(f.span.start as usize)
                }
                FactKind::Scope | FactKind::CtxRequirement => continue,
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
        // An enum variant (use or declaration) → its enum and value.
        if let Some(decl) = self.variant_ref_at(fid, offset) {
            if let Some(h) = self.variant_hover(decl) {
                return Some(h);
            }
        }
        // A signature param binding (or an unbound bare/inline param) → the
        // predicate it generates, with the bound field's signature when the cursor
        // is on the column/edge ident itself.
        if let Some(h) = self.binding_hover(fid, offset) {
            return Some(h);
        }
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

    /// The predicate a signature param binding generates, when the cursor sits on
    /// the binding — the column/edge ident or the operator/arrow between it and the
    /// param head. On the ident itself the bound field's signature leads. An
    /// *unbound* param of a bare/inline query carries the derived same-name
    /// equality (`status: text` → `status = $status`), anchored at the param name.
    fn binding_hover(&self, fid: usize, offset: u32) -> Option<String> {
        let within = |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;
        for d in &self.decls {
            let Decl::Query(q) = d else { continue };
            let root = match &q.body {
                QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                _ => self.query_root(q),
            };
            let Some(root) = root else { continue };
            for p in &q.params {
                // The binding region runs from the end of the param head (its type
                // annotation, or the bare name) through the bound ident, so the
                // `->` / operator token between them is hoverable too.
                let head_end = p.ty.as_ref().map(|t| t.span.end).unwrap_or(p.name.span.end);
                match &p.binding {
                    Some(ParamBinding::Edge(edge)) => {
                        if edge.span.file.0 as usize == fid
                            && head_end <= offset
                            && offset < edge.span.end
                        {
                            let line = format!(
                                "binds `{} = ${}` — via the `{}` relation edge",
                                edge.node, p.name.node, edge.node
                            );
                            return Some(self.with_field_sig(root, edge, within, line));
                        }
                    }
                    Some(ParamBinding::ColOp { op, col }) => {
                        if col.span.file.0 as usize == fid
                            && head_end <= offset
                            && offset < col.span.end
                        {
                            let line = format!(
                                "binds `{} {} ${}` — {}",
                                col.node,
                                op_str(*op),
                                p.name.node,
                                op_gloss(*op)
                            );
                            return Some(self.with_field_sig(root, col, within, line));
                        }
                    }
                    // Bare/inline queries bind an unbound param to its same-named
                    // column; block/raw queries reference params via `$`, so no fact.
                    None => {
                        if within(p.name.span)
                            && matches!(q.body, QueryBody::Bare | QueryBody::Inline(_))
                        {
                            let slice = std::slice::from_ref(&p.name);
                            if self.walk_path(root, slice).is_some() {
                                let line = format!(
                                    "binds `{n} = ${n}` — an unbound param binds its same-named column",
                                    n = p.name.node
                                );
                                return Some(self.with_field_sig(root, &p.name, within, line));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Prepend `ident`'s field signature to `line` when the cursor is on the ident
    /// and it resolves against `root`; the bare binding line otherwise.
    fn with_field_sig(
        &self,
        root: &str,
        ident: &Ident,
        within: impl Fn(Span) -> bool,
        line: String,
    ) -> String {
        if within(ident.span) {
            if let Some(f) = self.walk_path(root, std::slice::from_ref(ident)) {
                return format!("{}\n\n{line}", field_hover(f));
            }
        }
        line
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
    /// rooted at. Covers shape bodies, query `where`/`order` clauses, signature
    /// param bindings (`-> edge` / `op col`), and mutation write `where`/assign
    /// columns — the contexts whose root model is statically known. (Filters are
    /// omitted: their root is the polymorphic call site.)
    fn field_paths(&self) -> Vec<(&str, &[Ident])> {
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                Decl::Shape(s) => self.shape_paths(s.from.node.as_str(), &s.body, &mut out),
                Decl::Query(q) => {
                    // Clauses and bindings both root at the query's target model:
                    // the explicit statement target of a block, else the inferred
                    // target (its return).
                    let root = match &q.body {
                        QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                        _ => self.query_root(q),
                    };
                    let Some(root) = root else { continue };
                    for p in &q.params {
                        match &p.binding {
                            Some(ParamBinding::Edge(id))
                            | Some(ParamBinding::ColOp { col: id, .. }) => {
                                out.push((root, std::slice::from_ref(id)))
                            }
                            None => {}
                        }
                    }
                    match &q.body {
                        QueryBody::Block(stmt) => {
                            for c in &stmt.clauses {
                                clause_paths(c, root, &mut out);
                            }
                        }
                        QueryBody::Inline(clauses) => {
                            for c in clauses {
                                clause_paths(c, root, &mut out);
                            }
                        }
                        // A raw body is opaque SQL — no field paths inside.
                        QueryBody::Bare | QueryBody::Raw(_) => {}
                    }
                }
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
                // `field -> Shape`: the field is a relation reference; the shape name
                // is a type reference (collect_type_refs), not a field path.
                ShapeField::NestRef { field, .. } => {
                    out.push((from, std::slice::from_ref(field)));
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

    // ---- Enum variant navigation ------------------------------------------

    /// The declaration span of the enum variant the cursor sits on — either a variant
    /// use in value/default position (`where status = paid`, `default pending`) or the
    /// variant's own declaration inside its `enum` body. `None` off any variant. A use is
    /// resolved as a variant only when the compared/assigned column (or the field whose
    /// `default` it is) resolves to an enum type, so a same-named identifier elsewhere is
    /// never mistaken for one; variants are enum-local, so the returned span identifies
    /// exactly one enum's variant.
    fn variant_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;
        // The cursor on a variant declaration resolves to itself.
        for d in &self.decls {
            if let Decl::Enum(e) = d {
                for v in &e.variants {
                    if under(v.name.span) {
                        return Some(v.name.span);
                    }
                }
            }
        }
        // The cursor on a variant use resolves to its declaration.
        for (seg, enum_name) in self.variant_use_sites() {
            if under(seg.span) {
                return self.variant_decl_span(enum_name, &seg.node);
            }
        }
        None
    }

    /// Every enum-variant use site in value/default position, paired with the enum it
    /// belongs to: a `where`/write comparison whose column is enum-typed, a write assign
    /// to an enum column, and an enum field's `default <variant>`. The `Ident` is the
    /// variant token (so its span is the use); the `&str` is the owning enum name.
    fn variant_use_sites(&self) -> Vec<(&Ident, &str)> {
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    for mem in &m.members {
                        let Member::Field(f) = mem else { continue };
                        let Some(en) = self.field_enum(f) else {
                            continue;
                        };
                        for md in &f.modifiers {
                            if let Modifier::Default(based_ast::DefaultVal::Variant(v)) = md {
                                out.push((v, en));
                            }
                        }
                    }
                }
                Decl::Query(q) => {
                    let root = match &q.body {
                        QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                        QueryBody::Inline(_) => self.query_root(q),
                        QueryBody::Bare | QueryBody::Raw(_) => None,
                    };
                    if let Some(root) = root {
                        match &q.body {
                            QueryBody::Block(stmt) => {
                                for c in &stmt.clauses {
                                    self.clause_variant_sites(c, root, &mut out);
                                }
                            }
                            QueryBody::Inline(cs) => {
                                for c in cs {
                                    self.clause_variant_sites(c, root, &mut out);
                                }
                            }
                            QueryBody::Bare | QueryBody::Raw(_) => {}
                        }
                    }
                }
                Decl::Mutation(m) => self.write_variant_sites(&m.body, &mut out),
                _ => {}
            }
        }
        out
    }

    fn clause_variant_sites<'a>(
        &'a self,
        c: &'a Clause,
        root: &str,
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        if let Clause::Where(p) = c {
            self.pred_variant_sites(p, root, out);
        }
    }

    fn pred_variant_sites<'a>(
        &'a self,
        p: &'a Predicate,
        root: &str,
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        match p {
            Predicate::Or(a, b) | Predicate::And(a, b) => {
                self.pred_variant_sites(a, root, out);
                self.pred_variant_sites(b, root, out);
            }
            Predicate::Not(inner) => self.pred_variant_sites(inner, root, out),
            Predicate::Cmp { path, value, .. } => {
                if let (Some(en), Value::Path(vp)) =
                    (self.enum_of_path(root, &path.segments), value)
                {
                    if vp.segments.len() == 1 {
                        out.push((&vp.segments[0], en));
                    }
                }
            }
            Predicate::InList { path, values } => {
                if let Some(en) = self.enum_of_path(root, &path.segments) {
                    for v in values {
                        if let Value::Path(vp) = v {
                            if vp.segments.len() == 1 {
                                out.push((&vp.segments[0], en));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn write_variant_sites<'a>(
        &'a self,
        body: &'a [WriteStmt],
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        for w in body {
            match w {
                WriteStmt::Create { model, assigns } => {
                    self.assign_variant_sites(model.node.as_str(), assigns, out)
                }
                WriteStmt::Update {
                    model,
                    where_,
                    assigns,
                } => {
                    self.pred_variant_sites(where_, model.node.as_str(), out);
                    self.assign_variant_sites(model.node.as_str(), assigns, out);
                }
                WriteStmt::Delete { model, where_ }
                | WriteStmt::Restore { model, where_ }
                | WriteStmt::HardDelete { model, where_ } => {
                    self.pred_variant_sites(where_, model.node.as_str(), out)
                }
                WriteStmt::Tx(inner) => self.write_variant_sites(inner, out),
                WriteStmt::Raw(_) => {}
            }
        }
    }

    fn assign_variant_sites<'a>(
        &'a self,
        model: &str,
        assigns: &'a [Assign],
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        for a in assigns {
            if let (Some(en), Value::Path(vp)) = (
                self.enum_of_path(model, std::slice::from_ref(&a.col)),
                &a.value,
            ) {
                if vp.segments.len() == 1 {
                    out.push((&vp.segments[0], en));
                }
            }
        }
    }

    /// The enum a dotted column path (rooted at `root`) terminates on, or `None` when the
    /// terminal column is not enum-typed.
    fn enum_of_path<'a>(&'a self, root: &str, segs: &[Ident]) -> Option<&'a str> {
        self.field_enum(self.walk_path(root, segs)?)
    }

    /// The enum name a field is typed by, when its `UpperCamel` type resolves to a
    /// declared enum (not a model relation).
    fn field_enum<'a>(&'a self, f: &'a Field) -> Option<&'a str> {
        if let BaseType::Model(t) = &f.ty.base {
            if self.is_enum_decl(&t.node) {
                return Some(t.node.as_str());
            }
        }
        None
    }

    fn is_enum_decl(&self, name: &str) -> bool {
        self.decls
            .iter()
            .any(|d| matches!(d, Decl::Enum(e) if e.name.node == name))
    }

    /// The declaration span of variant `variant` in enum `enum_name`, or `None`.
    fn variant_decl_span(&self, enum_name: &str, variant: &str) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Enum(e) if e.name.node == enum_name => e
                .variants
                .iter()
                .find(|v| v.name.node == variant)
                .map(|v| v.name.span),
            _ => None,
        })
    }

    /// The `(enum_name, variant_name)` a declaration span identifies, when it is a variant
    /// declaration. Lets find-references key variant uses to the right enum.
    fn variant_of_decl_span(&self, target: Span) -> Option<(&str, &str)> {
        for d in &self.decls {
            if let Decl::Enum(e) = d {
                for v in &e.variants {
                    if v.name.span == target {
                        return Some((e.name.node.as_str(), v.name.node.as_str()));
                    }
                }
            }
        }
        None
    }

    /// A model or shape decl's one-line hover, by name.
    fn decl_hover_by_name(&self, name: &str) -> Option<String> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == name => Some(model_hover(m)),
            Decl::Shape(s) if s.name.node == name => Some(shape_hover(s)),
            Decl::Enum(e) if e.name.node == name => Some(enum_hover(e)),
            _ => None,
        })
    }

    /// An enum variant's hover: `` variant `paid` of `Status` `` (plus its explicit
    /// value, when written). `decl` is the variant's declaration span.
    fn variant_hover(&self, decl: Span) -> Option<String> {
        for d in &self.decls {
            let Decl::Enum(e) = d else { continue };
            for v in &e.variants {
                if v.name.span == decl {
                    let val = match v.value.as_ref().map(|s| &s.node) {
                        Some(VariantValue::Str(s)) => format!(" = \"{s}\""),
                        Some(VariantValue::Int(n)) => format!(" = {n}"),
                        None => String::new(),
                    };
                    return Some(format!(
                        "```based\nvariant {}{val}\n```\nvariant of enum `{}`",
                        v.name.node, e.name.node
                    ));
                }
            }
        }
        None
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
                // Enum → Enum; its variants → EnumMember children.
                Decl::Enum(e) if here(e.span) => {
                    let children = e
                        .variants
                        .iter()
                        .map(|v| symbol(&v.name, SymbolKind::ENUM_MEMBER, v.name.span, idx, None))
                        .collect();
                    out.push(symbol(
                        &e.name,
                        SymbolKind::ENUM,
                        e.span,
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

    /// Workspace symbols (`workspace/symbol`, ⌘T): every named declaration across the
    /// whole project — models (with their fields), shapes, scopes, queries, mutations,
    /// filters — filtered by a case-insensitive fuzzy subsequence match on `query`
    /// (empty query = everything). Unlike [`Snapshot::document_symbols`] this spans
    /// every file in the snapshot; each symbol carries its own file `Location` so the
    /// editor jumps straight to the declaration.
    pub fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let mut out = Vec::new();
        let mut push = |name: &Ident, kind: SymbolKind, container: Option<&str>| {
            if !fuzzy_match(query, &name.node) {
                return;
            }
            let fid = name.span.file.0 as usize;
            let Some((path, _)) = self.sources.get(fid) else {
                return;
            };
            let Ok(uri) = Url::from_file_path(path) else {
                return;
            };
            let location = Location::new(uri, span_range(name.span, &self.lines[fid]));
            out.push(sym_info(&name.node, kind, location, container));
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    push(&m.name, SymbolKind::STRUCT, None);
                    for mem in &m.members {
                        if let Member::Field(f) = mem {
                            push(&f.name, SymbolKind::FIELD, Some(&m.name.node));
                        }
                    }
                }
                Decl::Shape(s) => push(&s.name, SymbolKind::INTERFACE, None),
                Decl::Scope(s) => push(&s.name, SymbolKind::NAMESPACE, None),
                Decl::Enum(e) => {
                    push(&e.name, SymbolKind::ENUM, None);
                    for v in &e.variants {
                        push(&v.name, SymbolKind::ENUM_MEMBER, Some(&e.name.node));
                    }
                }
                Decl::Query(q) => push(&q.name, SymbolKind::FUNCTION, None),
                Decl::Mutation(m) => push(&m.name, SymbolKind::METHOD, None),
                Decl::Filter(f) => push(&f.name, SymbolKind::FUNCTION, None),
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

    /// Return type position: models and shapes (a callable may return either), plus
    /// the `stream` return form.
    fn return_type_items(&self) -> Vec<CompletionItem> {
        let mut items = vec![item("stream", CompletionItemKind::KEYWORD)];
        items.extend(self.model_name_items());
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
    "stream",
    "raw",
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

/// Whether `s` is a well-formed identifier — a rename target must be one, else the
/// edit would produce unparseable source. (Casing rules, e.g. models UpperName, are
/// left to sema, which re-flags a bad rename inline.)
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The identifier extent (byte range) the cursor at `offset` sits in, or `None` off
/// any word. Identifiers are ASCII, so a non-ASCII byte (high bit set) stops the walk
/// — a byte scan over identifier characters is safe. Shared by prepareRename (the
/// range it offers) and selection ranges (the token level).
fn word_extent(src: &str, offset: u32) -> Option<(usize, usize)> {
    let bytes = src.as_bytes();
    let off = (offset as usize).min(src.len());
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = off;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = off;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    (start != end).then_some((start, end))
}

/// A top-level declaration's extent span (name/decorators through the closing
/// delimiter) — the anchor folding and selection ranges expand to.
fn decl_span(d: &Decl) -> Span {
    match d {
        Decl::Model(m) => m.span,
        Decl::Shape(s) => s.span,
        Decl::Scope(s) => s.span,
        Decl::Enum(e) => e.span,
        Decl::Query(q) => q.span,
        Decl::Mutation(m) => m.span,
        Decl::Filter(f) => f.span,
    }
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

/// One `workspace/symbol` result: a flat, file-located symbol (no nesting — that is
/// what `container_name` is for).
#[allow(deprecated)] // `deprecated` is a required struct field, set to None.
fn sym_info(
    name: &str,
    kind: SymbolKind,
    location: Location,
    container: Option<&str>,
) -> SymbolInformation {
    SymbolInformation {
        name: name.to_owned(),
        kind,
        tags: None,
        deprecated: None,
        location,
        container_name: container.map(str::to_owned),
    }
}

/// Case-insensitive fuzzy subsequence match, the ⌘T convention: every char of
/// `query` must appear in `name` in order (not necessarily contiguously). An empty
/// query matches everything. The client re-ranks; this is the coarse server filter.
fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut chars = name.chars().flat_map(char::to_lowercase);
    for q in query.chars().flat_map(char::to_lowercase) {
        if !chars.any(|c| c == q) {
            return false;
        }
    }
    true
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
            Decl::Shape(s) => {
                out.push(&s.from);
                collect_shape_body_refs(&s.body, &mut out);
            }
            Decl::Scope(s) => {
                for t in &s.terms {
                    collect_type_expr(&t.ty, &mut out);
                }
            }
            Decl::Query(q) => {
                if !q.ret.ack {
                    out.push(&q.ret.ty);
                }
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
                // `-> ok` is the ack token, not a type reference.
                if !m.ret.ack {
                    out.push(&m.ret.ty);
                }
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
            // An enum decl has no outgoing type references (its variants are its own).
            Decl::Enum(_) => {}
        }
    }
    out
}

/// The `field -> Shape` references in a shape body (recursing through inline nests) —
/// each names a shape decl, so it rides the type-reference index (go-to-def,
/// find-references, rename).
fn collect_shape_body_refs<'a>(body: &'a [ShapeField], out: &mut Vec<&'a Ident>) {
    for f in body {
        match f {
            ShapeField::Nest { body, .. } => collect_shape_body_refs(body, out),
            ShapeField::NestRef { shape, .. } => out.push(shape),
            ShapeField::Bare(_) | ShapeField::Rename { .. } => {}
        }
    }
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

/// Every `filter(...)` call-name identifier across the AST — the sites a name *invokes*
/// a declared filter (`Predicate::FilterCall`), found by walking every predicate a
/// query/mutation/filter carries. The reference-collection twin for filters.
fn collect_filter_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Query(q) => match &q.body {
                QueryBody::Inline(cs) => cs.iter().for_each(|c| clause_filter_refs(c, &mut out)),
                QueryBody::Block(stmt) => stmt
                    .clauses
                    .iter()
                    .for_each(|c| clause_filter_refs(c, &mut out)),
                QueryBody::Bare | QueryBody::Raw(_) => {}
            },
            Decl::Mutation(m) => write_filter_refs(&m.body, &mut out),
            Decl::Filter(f) => pred_filter_refs(&f.pred, &mut out),
            _ => {}
        }
    }
    out
}

/// Filter-call names in a query clause's `where` predicate.
fn clause_filter_refs<'a>(c: &'a Clause, out: &mut Vec<&'a Ident>) {
    if let Clause::Where(p) = c {
        pred_filter_refs(p, out);
    }
}

/// Filter-call names in a mutation write body's `where` predicates (recursing `tx`).
fn write_filter_refs<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a Ident>) {
    for w in body {
        match w {
            WriteStmt::Update { where_, .. }
            | WriteStmt::Delete { where_, .. }
            | WriteStmt::Restore { where_, .. }
            | WriteStmt::HardDelete { where_, .. } => pred_filter_refs(where_, out),
            WriteStmt::Tx(inner) => write_filter_refs(inner, out),
            WriteStmt::Create { .. } | WriteStmt::Raw(_) => {}
        }
    }
}

/// Filter-call names anywhere in a predicate tree.
fn pred_filter_refs<'a>(p: &'a Predicate, out: &mut Vec<&'a Ident>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_filter_refs(a, out);
            pred_filter_refs(b, out);
        }
        Predicate::Not(inner) => pred_filter_refs(inner, out),
        Predicate::FilterCall { name, .. } => out.push(name),
        Predicate::Cmp { .. }
        | Predicate::InList { .. }
        | Predicate::Bare(_)
        | Predicate::Raw(_) => {}
    }
}

/// Every explicit inverse pairing `(Model.field)` as its `(model, field)` idents — the
/// `field` part references a forward edge on `model`. Inferred inverses carry no such
/// written pairing (they surface via `Fact.nav` instead).
fn collect_explicit_inverse_fields(decls: &[Decl]) -> Vec<(&Ident, &Ident)> {
    let mut out = Vec::new();
    for d in decls {
        if let Decl::Model(m) = d {
            for mem in &m.members {
                if let Member::Field(f) = mem {
                    if let Some(inv) = &f.inverse {
                        out.push((&inv.model, &inv.field));
                    }
                }
            }
        }
    }
    out
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
        Predicate::InList { path, values } => {
            out.push((root, &path.segments));
            for v in values {
                value_paths(v, root, out);
            }
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

// ---- Param / `$ctx` reference collectors ------------------------------------

/// A callable's declared params, or an empty slice for a non-callable declaration.
fn decl_params(d: &Decl) -> &[Param] {
    match d {
        Decl::Query(q) => &q.params,
        Decl::Mutation(m) => &m.params,
        Decl::Filter(f) => &f.params,
        _ => &[],
    }
}

/// Every `$param` / `$ctx.field` reference in a callable's body (query clauses,
/// mutation writes, or a filter predicate) — the sites that name a param or bag field.
fn callable_param_refs(d: &Decl) -> Vec<&ParamRef> {
    let mut out = Vec::new();
    match d {
        Decl::Query(q) => match &q.body {
            QueryBody::Inline(cs) => cs.iter().for_each(|c| clause_param_refs(c, &mut out)),
            QueryBody::Block(stmt) => stmt
                .clauses
                .iter()
                .for_each(|c| clause_param_refs(c, &mut out)),
            QueryBody::Raw(r) => raw_param_refs(r, &mut out),
            QueryBody::Bare => {}
        },
        Decl::Mutation(m) => write_param_refs(&m.body, &mut out),
        Decl::Filter(f) => pred_param_refs(&f.pred, &mut out),
        _ => {}
    }
    out
}

fn clause_param_refs<'a>(c: &'a Clause, out: &mut Vec<&'a ParamRef>) {
    if let Clause::Where(p) = c {
        pred_param_refs(p, out);
    }
}

fn write_param_refs<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a ParamRef>) {
    for w in body {
        match w {
            WriteStmt::Create { assigns, .. } => {
                assigns.iter().for_each(|a| value_param_refs(&a.value, out))
            }
            WriteStmt::Update {
                where_, assigns, ..
            } => {
                pred_param_refs(where_, out);
                assigns.iter().for_each(|a| value_param_refs(&a.value, out));
            }
            WriteStmt::Delete { where_, .. }
            | WriteStmt::Restore { where_, .. }
            | WriteStmt::HardDelete { where_, .. } => pred_param_refs(where_, out),
            WriteStmt::Tx(inner) => write_param_refs(inner, out),
            WriteStmt::Raw(r) => raw_param_refs(r, out),
        }
    }
}

fn pred_param_refs<'a>(p: &'a Predicate, out: &mut Vec<&'a ParamRef>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_param_refs(a, out);
            pred_param_refs(b, out);
        }
        Predicate::Not(inner) => pred_param_refs(inner, out),
        Predicate::Cmp { value, .. } => value_param_refs(value, out),
        Predicate::InList { values, .. } => values.iter().for_each(|v| value_param_refs(v, out)),
        Predicate::FilterCall { args, .. } => args.iter().for_each(|v| value_param_refs(v, out)),
        Predicate::Raw(r) => raw_param_refs(r, out),
        Predicate::Bare(_) => {}
    }
}

fn value_param_refs<'a>(v: &'a Value, out: &mut Vec<&'a ParamRef>) {
    match v {
        Value::Param(pr) => out.push(pr),
        Value::Func(fc) => fc.args.iter().for_each(|a| value_param_refs(a, out)),
        _ => {}
    }
}

fn raw_param_refs<'a>(r: &'a RawSql, out: &mut Vec<&'a ParamRef>) {
    for part in &r.parts {
        if let RawPart::Param(pr) = part {
            out.push(pr);
        }
    }
}

/// The byte offset of the start of the line containing `off` (the char after the
/// preceding newline, or 0). Where a leading `@was` decorator line is inserted.
fn line_start(src: &str, off: u32) -> usize {
    let o = (off as usize).min(src.len());
    src[..o].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

// ---- Hover renderers ("what", rust-analyzer baseline) -----------------------

/// A `TypeExpr` as source writes it: base spelling + `?` (optional) + `[]` (many).
fn type_str(ty: &TypeExpr) -> String {
    let mut s = match &ty.base {
        BaseType::Primitive(p) => primitive_str(*p),
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
fn primitive_str(p: Primitive) -> String {
    match p {
        Primitive::Text => "text".into(),
        Primitive::Int => "int".into(),
        Primitive::Bool => "bool".into(),
        Primitive::Timestamp => "timestamp".into(),
        Primitive::Date => "date".into(),
        Primitive::Json => "json".into(),
        Primitive::Uuid => "uuid".into(),
        Primitive::Id => "Id".into(),
        Primitive::Float => "float".into(),
        Primitive::Decimal { precision, scale } => format!("decimal({precision}, {scale})"),
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

/// A binding operator's DSL spelling.
fn op_str(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "~",
        Op::In => "in",
        Op::Has => "has",
    }
}

/// What a binding operator means, for the binding hover.
fn op_gloss(op: Op) -> &'static str {
    match op {
        Op::Has => "containment (array/json); the column is the left operand",
        Op::In => "membership; the column is the left operand",
        Op::Like => "SQL `LIKE`, pattern passed verbatim; the column is the left operand",
        _ => "the column is the left operand",
    }
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

/// An enum's hover: `enum Name { a, b, c }` (variant names, a compact closed set).
fn enum_hover(e: &EnumDecl) -> String {
    let names = e
        .variants
        .iter()
        .map(|v| v.name.node.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("```based\nenum {} {{ {names} }}\n```", e.name.node)
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

/// A query's hover: `query name(params) -> Ret[]` (or `-> stream Ret`).
fn query_hover(q: &Query) -> String {
    let stream = if q.ret.stream { "stream " } else { "" };
    let card = if q.ret.many { "[]" } else { "" };
    format!(
        "```based\nquery {}({}) -> {stream}{}{card}\n```",
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

// ---- Offline migration-drift diagnostic -------------------------------------

/// The offline drift diagnostics for a project: the schema-vs-migrations delta the editor
/// can compute with no database. Empty unless the project has captured
/// migrations *and* the current `.bsl` has structural changes not yet in one. Each change
/// is anchored at the declaration it touches (a model's name) and the whole set is
/// reported with the total count ("N uncaptured schema changes — run `based migrate gen`").
/// A spent `@was` (a rename already captured) is flagged separately so it can be removed.
fn drift_diagnostics(
    root: &Path,
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
) -> Vec<Diagnostic> {
    use based_codegen::migrate;
    // Only projects that have opted into migrations get a drift check — no `migrations/`
    // (or an unreadable latest snapshot) means the author isn't tracking migrations yet.
    let Some(prev) = latest_snapshot(root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let steps = migrate::drift(&prev, schema);
    let total = steps.len();
    if total > 0 {
        // Group each step under the model it touches, so one diagnostic per changed
        // declaration lists its own changes; steps with no locatable model (a dropped
        // table, a scope-only change) fall back to the first model's name.
        let msg = format!(
            "{total} uncaptured schema change{} — run `based migrate gen`",
            if total == 1 { "" } else { "s" }
        );
        let mut by_span: HashMap<(u32, u32, u32), Vec<String>> = HashMap::new();
        let mut order: Vec<Span> = Vec::new();
        for step in &steps {
            let span = step
                .table_name()
                .and_then(|t| self_model_span(schema, decls, t))
                .or_else(|| first_model_span(decls));
            let Some(span) = span else { continue };
            let key = (span.file.0, span.start, span.end);
            if !by_span.contains_key(&key) {
                order.push(span);
            }
            by_span.entry(key).or_default().push(step.describe());
        }
        for span in order {
            let key = (span.file.0, span.start, span.end);
            let note = by_span.remove(&key).unwrap_or_default().join("; ");
            out.push(
                Diagnostic::warning(based_sema::code::MIGRATE_DRIFT, msg.clone())
                    .at(span)
                    .note(note),
            );
        }
    }

    out.extend(spent_was_diagnostics(&prev, schema, decls));
    out
}

/// Spent `@was` directives: a rename already reflected in the latest snapshot (the old
/// name is gone, the new name present), so the `@was` no longer does anything and should
/// be removed (`W0107`). Anchored at the `@was` literal.
fn spent_was_diagnostics(
    prev: &based_codegen::migrate::Snapshot,
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in decls {
        let Decl::Model(m) = d else { continue };
        let Some(rmodel) = schema.model(&m.name.node) else {
            continue;
        };
        // Field-level `@was`.
        for mem in &m.members {
            let Member::Field(f) = mem else { continue };
            let Some(was) = &f.was else { continue };
            let new_col = rmodel
                .member(&f.name.node)
                .map(|rm| rm.physical_col().to_string());
            if let (Some(t), Some(new_col)) = (prev.table(&rmodel.table), new_col) {
                if t.column(&was.node).is_none() && t.column(&new_col).is_some() {
                    out.push(spent_was(was.span, &was.node));
                }
            }
        }
        // Model-level `@was("old_table")`.
        for deco in &m.decorators {
            if deco.name.node != "was" {
                continue;
            }
            let Some(based_ast::DecoArg::Lit(based_ast::Literal::Str(old))) = deco.args.first()
            else {
                continue;
            };
            if prev.table(old).is_none() && prev.table(&rmodel.table).is_some() {
                out.push(spent_was(deco.span, old));
            }
        }
    }
    out
}

fn spent_was(span: Span, old: &str) -> Diagnostic {
    Diagnostic::warning(
        based_sema::code::WAS_SPENT,
        format!("`@was(\"{old}\")` rename already captured — remove it"),
    )
    .at(span)
}

/// The declaration-name span of the model whose (physical) table is `table`.
fn self_model_span(
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
    table: &str,
) -> Option<Span> {
    let name = schema
        .models
        .iter()
        .find(|m| m.table == table)?
        .name
        .clone();
    decls.iter().find_map(|d| match d {
        Decl::Model(m) if m.name.node == name => Some(m.name.span),
        _ => None,
    })
}

/// The first model declaration's name span — a fallback anchor for a drift step whose
/// table no longer has a declaration (a dropped model).
fn first_model_span(decls: &[Decl]) -> Option<Span> {
    decls.iter().find_map(|d| match d {
        Decl::Model(m) => Some(m.name.span),
        _ => None,
    })
}

/// The highest-numbered `migrations/NNNN_slug/schema.snap` under `root`, parsed — the
/// diff baseline for the drift check. `None` when there is no `migrations/` dir, no
/// migration in it, or the snapshot is unreadable/corrupt (drift stays silent then).
fn latest_snapshot(root: &Path) -> Option<based_codegen::migrate::Snapshot> {
    let dir = root.join("migrations");
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((num, _)) = name.split_once('_') {
            if let Ok(n) = num.parse::<u32>() {
                if best.as_ref().map(|(b, _)| n > *b).unwrap_or(true) {
                    best = Some((n, entry.path()));
                }
            }
        }
    }
    let (_, path) = best?;
    let text = std::fs::read_to_string(path.join("schema.snap")).ok()?;
    based_codegen::migrate::Snapshot::parse(&text).ok()
}

/// Compile the manifest project rooted at `root` (the dir holding `based.toml`),
/// with `overlays` (canonical path -> unsaved buffer text) taking precedence over
/// on-disk contents. Overlays for files outside this project are simply ignored.
pub fn compile_manifest(root: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    match based_manifest::discover(root) {
        Ok(project) => {
            let paths = project.files.into_iter().map(|f| f.path).collect();
            compile_paths(paths, overlays, Vec::new(), Some(root))
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
            compile_paths(ps, overlays, diags, Some(root))
        }
    }
}

/// Compile a single `.bsl` file under no manifest in isolation (the fallback for a
/// file that belongs to no project — cross-file references cannot resolve here).
pub fn compile_loose(file: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    compile_paths(vec![file.to_path_buf()], overlays, Vec::new(), None)
}

/// Read + parse + check a fixed file set, preferring open buffers over disk, into a
/// snapshot. `project_diagnostics` carries any spanless project-level issues.
fn compile_paths(
    paths: Vec<PathBuf>,
    overlays: &HashMap<PathBuf, String>,
    project_diagnostics: Vec<Diagnostic>,
    migrations_root: Option<&Path>,
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
    let mut checked = None;
    if parse_ok {
        let (schema, diags) = based_sema::check(&decls);
        diagnostics.extend(diags);
        facts = based_facts::facts(&schema, &decls);
        // Offline migration-drift diagnostic: if the project has captured migrations and
        // the `.bsl` has structural changes not yet in one, flag them.
        if let Some(root) = migrations_root {
            diagnostics.extend(drift_diagnostics(root, &schema, &decls));
        }
        checked = Some(schema);
    }

    let lines = sources.iter().map(|(_, s)| LineIndex::new(s)).collect();
    Snapshot {
        sources,
        lines,
        facts,
        decls,
        diagnostics,
        project_diagnostics,
        schema: checked,
        migrations_root: migrations_root.map(Path::to_path_buf),
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
    use std::collections::HashSet;
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
    /// not the opened workspace root.
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

    /// A `field -> Shape` nest reference resolves to the referenced shape decl's
    /// name span (cross-file), and the decl's references include the nest site.
    #[test]
    fn goto_definition_resolves_shape_nest_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Cursor mid-`UserRef` in `placed_by -> UserRef` (order/model.bsl). The last
        // occurrence is the shape field (earlier ones sit in comments).
        let ofid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[ofid].1;
        let off = (src.rfind("-> UserRef").unwrap() + 4) as u32;
        let def = snap
            .definition_at(ofid, off)
            .expect("shape reference resolves");
        let (def_path, def_src) = &snap.sources[def.file.0 as usize];
        assert!(def_path.ends_with("user/model.bsl"), "{def_path:?}");
        assert_eq!(&def_src[def.start as usize..def.end as usize], "UserRef");

        // Find-references from the decl lists the nest reference site.
        let ufid = def.file.0 as usize;
        let refs = snap.references_at(ufid, def.start, false);
        assert!(
            refs.iter().any(|s| s.file.0 as usize == ofid),
            "nest reference listed: {refs:?}"
        );
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

    /// Find-references on a forward edge includes the inverse that pairs through it —
    /// the "back-follow". `OrderItem.order`'s references include `Order.items` (the
    /// inferred inverse joining via `order`), plus the declaration itself when asked.
    #[test]
    fn references_include_inverse_back_edge() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Cursor on the `order` forward-edge declaration in order_item/model.bsl.
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let oi_src = &snap.sources[oi_fid].1;
        let off = (oi_src.find("order:").unwrap() + 1) as u32;

        let refs = snap.references_at(oi_fid, off, true);
        // The inverse `Order.items` (in order/model.bsl) is among the references.
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let model_src = &snap.sources[model_fid].1;
        assert!(
            refs.iter().any(|s| s.file.0 as usize == model_fid
                && &model_src[s.start as usize..s.end as usize] == "items"),
            "inverse back-edge `Order.items` should be a reference: {refs:?}"
        );
        // With include_declaration, the `order` field's own name is present too.
        assert!(
            refs.iter().any(|s| s.file.0 as usize == oi_fid
                && &oi_src[s.start as usize..s.end as usize] == "order"),
            "the declaration itself: {refs:?}"
        );
    }

    /// Find-references + go-to-def reach into query/mutation bodies: a field used in a
    /// query `where` and a `filter(...)` call are both resolved.
    #[test]
    fn references_reach_query_bodies_and_filters() {
        let ws = TempWorkspace::new("refs");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { active: bool  qty: int? }\n\
             filter big(n: int) = qty > $n;\n\
             shape W from Widget { active }\n\
             query find() -> W[] { list Widget where (active and big(5)); }\n",
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

        // Go-to-def on the `big(5)` filter call → the `filter big` declaration.
        let call = (src.find("big(5)").unwrap() + 1) as u32;
        let def = snap.definition_at(fid, call).expect("filter call resolves");
        assert_eq!(&src[def.start as usize..def.end as usize], "big");
        assert_eq!(def.start as usize, src.find("big(n").unwrap());

        // Find-references on the `filter big` declaration → the call site.
        let decl = (src.find("big(n").unwrap() + 1) as u32;
        let frefs = snap.references_at(fid, decl, false);
        assert!(
            frefs
                .iter()
                .any(|s| s.start as usize == src.find("big(5)").unwrap()),
            "the filter call site: {frefs:?}"
        );

        // Find-references on the `active` field → its use in the query `where`.
        let afield = (src.find("active: bool").unwrap() + 1) as u32;
        let arefs = snap.references_at(fid, afield, false);
        assert!(
            arefs
                .iter()
                .any(|s| s.start as usize == src.find("active and").unwrap()),
            "the `where` use of active: {arefs:?}"
        );
    }

    /// Rename rewrites every occurrence spelling the old name across files — a model's
    /// declaration and its cross-file type references — but never the differently-named
    /// inverse back-edge that only pairs through it.
    fn rename_texts(snap: &Snapshot, changes: &HashMap<Url, Vec<TextEdit>>) -> Vec<String> {
        changes
            .iter()
            .flat_map(|(uri, edits)| {
                let fid = snap.file_id_of(&uri.to_file_path().unwrap()).unwrap();
                let src = snap.sources[fid].1.clone();
                let idx = &snap.lines[fid];
                edits.iter().map(move |e| {
                    let s = idx.offset(e.range.start);
                    let en = idx.offset(e.range.end);
                    src[s..en].to_string()
                })
            })
            .collect()
    }

    #[test]
    fn rename_model_rewrites_declaration_and_cross_file_refs() {
        let ws = TempWorkspace::new("rename_model");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Cursor on the `Org` reference in user.bsl → rename to `Organization`.
        let ufid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let off = (snap.sources[ufid].1.find("Org").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(ufid, off, "Organization")
            .expect("model reference is renameable");

        // Both files carry an edit: the decl (org.bsl) and the type ref (user.bsl).
        let ofid = snap.file_id_of(&ws.path("org.bsl")).unwrap();
        let ouri = Url::from_file_path(&snap.sources[ofid].0).unwrap();
        let uuri = Url::from_file_path(&snap.sources[ufid].0).unwrap();
        assert!(changes.contains_key(&ouri), "declaration file edited");
        assert!(changes.contains_key(&uuri), "reference file edited");

        // Every rewritten site spelled the old name `Org` (never `name`, `User`, …).
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts.len(), 2, "decl + one reference: {texts:?}");
        assert!(texts.iter().all(|t| t == "Org"), "{texts:?}");
        // The new text is the requested name.
        assert!(changes
            .values()
            .flatten()
            .all(|e| e.new_text == "Organization"));
    }

    #[test]
    fn rename_forward_edge_leaves_inverse_back_edge_untouched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Rename the `order` forward edge on OrderItem. Find-references lists the
        // inverse `Order.items` as a related site, but rename must not touch it (it
        // spells `items`, a different name).
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let off = (snap.sources[oi_fid].1.find("order:").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(oi_fid, off, "parent")
            .expect("forward edge is renameable");
        let texts = rename_texts(&snap, &changes);
        assert!(
            texts.iter().all(|t| t == "order"),
            "only `order` sites rewritten, not the `items` back-edge: {texts:?}"
        );
        assert!(
            !texts.is_empty() && texts.iter().any(|t| t == "order"),
            "the declaration itself is rewritten: {texts:?}"
        );
    }

    #[test]
    fn rename_rejects_bad_target_and_non_symbol_cursor() {
        let ws = TempWorkspace::new("rename_reject");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("org.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // A non-identifier new name is refused (would produce unparseable source).
        let decl = (src.find("Org").unwrap() + 1) as u32;
        assert!(snap.rename_edits(fid, decl, "1bad").is_none());
        assert!(snap.rename_edits(fid, decl, "has space").is_none());

        // The cursor on a primitive keyword (`text`) is not a renameable symbol.
        let prim = (src.find("text").unwrap() + 1) as u32;
        assert!(snap.rename_edits(fid, prim, "blob").is_none());
        assert!(snap.prepare_rename_range(fid, prim).is_none());

        // prepareRename offers the identifier extent on the declaration.
        let r = snap
            .prepare_rename_range(fid, decl)
            .expect("Org is renameable");
        assert_eq!(snap.lines[fid].offset(r.start), src.find("Org").unwrap());
        assert_eq!(
            snap.lines[fid].offset(r.end),
            src.find("Org").unwrap() + "Org".len()
        );
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

    /// A `-> stream Shape` query's hover shows the stream return form.
    #[test]
    fn hover_shows_the_stream_return_form() {
        let ws = TempWorkspace::new("stream_hover");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "@sort(total desc)\n\
             Order { status: text  total: int }\n\
             shape OrderRow from Order { status  total }\n\
             query export_orders() -> stream OrderRow;\n",
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
        let off = (src.find("export_orders").unwrap() + 1) as u32;
        let h = snap.hover_at(fid, off).expect("query hover");
        assert!(h.contains("-> stream OrderRow"), "{h}");
    }

    /// An `-> ok` mutation's hover shows the ack return form as written; the `ok`
    /// token itself is not a type reference (hovering it resolves nothing, no crash).
    #[test]
    fn hover_shows_the_ack_return_form() {
        let ws = TempWorkspace::new("ack_hover");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Tag { label: text }\n\
             mutation drop_tag(id: Id) -> ok { delete Tag where (id = $id) }\n",
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
        let off = (src.find("drop_tag").unwrap() + 1) as u32;
        let h = snap.hover_at(fid, off).expect("mutation hover");
        assert!(h.contains("-> ok"), "{h}");
        let ok_off = (src.find("-> ok").unwrap() + 3) as u32;
        assert!(snap.hover_at(fid, ok_off).is_none());
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

    /// Folding ranges cover each multi-line declaration body: the `Order` model folds
    /// from its `{` header line to its closing brace, the `OrderCard` shape likewise,
    /// while the single-line `scope Tenant (…)` yields no fold.
    #[test]
    fn folding_ranges_over_commerce_order_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let idx = &snap.lines[fid];
        let ranges = snap.folding_ranges(fid);

        // The `Order { … }` model: fold from the `{` header line to its `}` line.
        let open = src.find("Order {").unwrap() + "Order ".len();
        let close = src[open..].find('}').map(|i| open + i).unwrap();
        let open_line = idx.position(open).line;
        let close_line = idx.position(close).line;
        assert!(close_line > open_line);
        assert!(
            ranges.iter().any(|r| r.start_line == open_line
                && r.end_line == close_line
                && r.kind == Some(FoldingRangeKind::Region)),
            "Order body should fold {open_line}..{close_line}: {ranges:?}"
        );

        // The single-line `scope Tenant (…)` decl has nothing to fold.
        let scope_line = idx.position(src.find("scope Tenant").unwrap()).line;
        assert!(
            !ranges.iter().any(|r| r.start_line == scope_line),
            "a single-line decl must not fold: {ranges:?}"
        );

        // Both the model and the shape fold (two multi-line decls in the file).
        assert!(ranges.len() >= 2, "{ranges:?}");
    }

    /// `format_document` is a no-op on the canonical commerce file, and rewrites an
    /// overlaid messy buffer into that same canonical layout.
    #[test]
    fn format_document_noop_then_reformats_overlay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let model = root.join("order/model.bsl");

        // Canonical on disk: no edit to apply.
        let snap = compile_manifest(&root, &HashMap::new());
        let fid = snap.file_id_of(&model).unwrap();
        let canonical = snap.sources[fid].1.clone();
        assert_eq!(snap.format_document(fid), None);

        // Overlay a messy buffer for the same file: it formats back to the canonical text.
        let mut overlays = HashMap::new();
        let messy = "Order {\n  deleted_at:timestamp?\n     org: Org\n}\n";
        overlays.insert(std::fs::canonicalize(&model).unwrap(), messy.to_string());
        let snap = compile_manifest(&root, &overlays);
        let fid = snap.file_id_of(&model).unwrap();
        let formatted = snap.format_document(fid).expect("messy buffer reformats");
        assert_eq!(
            formatted,
            "Order {\n  deleted_at: timestamp?\n  org:        Org\n}\n"
        );
        assert_ne!(formatted, canonical); // the overlay replaced the on-disk model
    }

    /// A selection range expands outward through the AST: the `total` token → its
    /// field declaration → the enclosing `Order` model → the whole file, each range
    /// strictly containing the one it parents.
    #[test]
    fn selection_range_expands_token_to_field_to_model_to_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let idx = &snap.lines[fid];

        // Cursor inside the `total: int` field name.
        let off = (src.find("total:").unwrap() + 1) as u32;
        let sr = snap
            .selection_range(fid, off)
            .expect("a selection hierarchy");

        // Walk the chain innermost → outermost, capturing each level's source text.
        let mut texts = Vec::new();
        let mut cur = Some(&sr);
        while let Some(node) = cur {
            let s = idx.offset(node.range.start);
            let e = idx.offset(node.range.end);
            texts.push(src[s..e].to_string());
            cur = node.parent.as_deref();
        }

        // Innermost is the token; a field level carries `total` + its `decimal` type; a
        // model level spans the whole `Order` body; the outermost is the whole file.
        assert_eq!(texts.first().unwrap(), "total");
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("total") && t.contains("decimal")),
            "a field-level range: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Order {") && t.contains("items")),
            "a model-level range: {texts:?}"
        );
        assert_eq!(
            texts.last().unwrap().len(),
            src.len(),
            "outermost is the whole file"
        );

        // Strictly nesting: each level is wider than the one it parents.
        for w in texts.windows(2) {
            assert!(
                w[0].len() < w[1].len(),
                "ranges must strictly nest: {texts:?}"
            );
        }
    }

    /// Workspace symbols span every file in the project (unlike document symbols),
    /// map each declaration kind, nest fields under their model via `container_name`,
    /// and filter by a fuzzy subsequence query. Asserted over the commerce schema.
    #[test]
    fn workspace_symbols_span_project_and_filter() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);

        // Empty query = everything. Symbols come from many files, each carrying its
        // own file `Location` (workspace-wide, not one file).
        let all = snap.workspace_symbols("");
        let files: HashSet<_> = all.iter().map(|s| s.location.uri.to_string()).collect();
        assert!(files.len() > 1, "symbols should span multiple files");

        // The `Order` model (Struct) and its `status` field (Field, contained in Order).
        let order = all
            .iter()
            .find(|s| s.name == "Order")
            .expect("Order symbol");
        assert_eq!(order.kind, SymbolKind::STRUCT);
        let status = all
            .iter()
            .find(|s| s.name == "status" && s.container_name.as_deref() == Some("Order"))
            .expect("Order.status field");
        assert_eq!(status.kind, SymbolKind::FIELD);

        // Kind mapping across decl kinds: shape → Interface, query → Function,
        // mutation → Method.
        assert_eq!(
            all.iter().find(|s| s.name == "OrderCard").unwrap().kind,
            SymbolKind::INTERFACE
        );
        assert_eq!(
            all.iter().find(|s| s.name == "place_order").unwrap().kind,
            SymbolKind::METHOD
        );
        assert_eq!(
            all.iter().find(|s| s.name == "my_org_orders").unwrap().kind,
            SymbolKind::FUNCTION
        );

        // Fuzzy subsequence filter, case-insensitive: "oc" matches OrderCard (O..C),
        // and the query narrows the set. A non-subsequence query drops it.
        let oc = snap.workspace_symbols("oc");
        assert!(oc.iter().any(|s| s.name == "OrderCard"));
        assert!(oc.len() < all.len());
        assert!(snap
            .workspace_symbols("zzq")
            .iter()
            .all(|s| s.name != "OrderCard"));
    }

    /// `fuzzy_match` is an ordered, case-insensitive subsequence test; empty matches all.
    #[test]
    fn fuzzy_match_is_ordered_subsequence() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("oc", "OrderCard"));
        assert!(fuzzy_match("order", "Order"));
        assert!(fuzzy_match("PO", "place_order"));
        assert!(!fuzzy_match("co", "OrderCard")); // out of order
        assert!(!fuzzy_match("xyz", "Order"));
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
    /// An added column not yet in the latest `schema.snap` surfaces as an offline drift
    /// diagnostic (W0108) anchored at the changed model.
    #[test]
    fn uncaptured_change_is_a_drift_diagnostic() {
        let ws = TempWorkspace::new("drift");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text?\n}\n",
        );
        // The captured snapshot knows only `name` — `barcode` is uncaptured.
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column name text not_null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");

        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            snap.diagnostics.iter().any(|d| d.code == "W0108"),
            "expected W0108 drift, got {:?}",
            snap.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    /// When the snapshot already matches the schema, there is no drift diagnostic; and a
    /// spent `@was` (rename already captured) surfaces as W0107.
    #[test]
    fn captured_schema_has_no_drift_but_flags_spent_was() {
        let ws = TempWorkspace::new("nodrift");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  barcode: text? @was(\"upc\")\n}\n",
        );
        // Snapshot already has `barcode` (the rename was captured) — no `upc` to rename.
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column barcode text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");

        let snap = compile_manifest(&ws.root, &HashMap::new());
        let codes: Vec<&str> = snap.diagnostics.iter().map(|d| d.code).collect();
        assert!(!codes.contains(&"W0108"), "unexpected drift: {codes:?}");
        assert!(
            codes.contains(&"W0107"),
            "expected spent-@was W0107: {codes:?}"
        );
    }

    /// Apply a file's rename edits to its source, returning the rewritten text.
    /// Edits are non-overlapping (a zero-width `@was` insertion may share a start
    /// offset with the name replacement); applying highest-offset-first, and for an
    /// equal start the wider replacement before the empty insertion, lands the
    /// inserted decorator before the renamed name.
    fn apply_edits(snap: &Snapshot, fid: usize, edits: &[TextEdit]) -> String {
        let idx = &snap.lines[fid];
        let mut spans: Vec<(usize, usize, &str)> = edits
            .iter()
            .map(|e| {
                (
                    idx.offset(e.range.start),
                    idx.offset(e.range.end),
                    e.new_text.as_str(),
                )
            })
            .collect();
        spans.sort_by_key(|(s, e, _)| (*s, *e));
        let mut out = snap.sources[fid].1.clone();
        for (s, e, t) in spans.into_iter().rev() {
            out.replace_range(s..e, t);
        }
        out
    }

    /// (a) A callable param renames its declaration and every `$param` use in *that*
    /// callable's body — and only that callable's (params are callable-local).
    #[test]
    fn rename_param_rewrites_decl_and_local_uses_only() {
        let ws = TempWorkspace::new("rename_param");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { qty: int? }\n\
             shape W from Widget { qty }\n\
             query find(min: int) -> W[] { list Widget where (qty > $min); }\n\
             query other(min: int) -> W[] { list Widget where (qty < $min); }\n",
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

        // Cursor on the `$min` use in `find` → rename to `floor`.
        let use_off = (src.find("qty > $min").unwrap() + "qty > $".len()) as u32;
        let changes = snap
            .rename_edits(fid, use_off, "floor")
            .expect("param is renameable");
        let texts = rename_texts(&snap, &changes);
        // The decl `min` + its one body use — both spelling `min`, nothing from `other`.
        assert_eq!(texts.len(), 2, "decl + local use only: {texts:?}");
        assert!(texts.iter().all(|t| t == "min"), "{texts:?}");

        // Applied: `find`'s param + use become `floor`; `other`'s `min` is untouched.
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("query find(floor: int)"), "{out}");
        assert!(out.contains("qty > $floor"), "{out}");
        assert!(out.contains("query other(min: int)"), "{out}");
        assert!(out.contains("qty < $min"), "{out}");
    }

    /// (b) A `$ctx.<field>` bag field renames its `scope … = $ctx.field` binding and
    /// every callable use, leaving the scope *column* and same-named model columns alone.
    #[test]
    fn rename_ctx_field_rewrites_binding_and_uses() {
        let ws = TempWorkspace::new("rename_ctx");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "scope Tenant (org: Org = $ctx.org)\n\
             Org { name: text }\n\
             @scope Tenant\n\
             Widget { org: Org  name: text }\n\
             query mine() -> Widget[] scoped Tenant { list Widget where (org = $ctx.org); }\n",
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

        // Cursor on the query's `$ctx.org` field segment → rename bag field to `tenant`.
        let off = (src.find("= $ctx.org)").unwrap() + "= $ctx.".len()) as u32;
        let changes = snap
            .rename_edits(fid, off, "tenant")
            .expect("ctx field is renameable");
        let texts = rename_texts(&snap, &changes);
        // The scope-term binding and the query use — two `org` segments, nothing else.
        assert_eq!(texts.len(), 2, "scope binding + query use: {texts:?}");
        assert!(texts.iter().all(|t| t == "org"), "{texts:?}");

        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // Both `$ctx.org` become `$ctx.tenant`; the scope column `org:` and the model
        // field `org: Org` (and the `where` LHS `org`) keep their name.
        assert!(
            out.contains("scope Tenant (org: Org = $ctx.tenant)"),
            "{out}"
        );
        assert!(out.contains("where (org = $ctx.tenant)"), "{out}");
        assert!(out.contains("Widget { org: Org"), "{out}");
    }

    /// (c) A query name is a wire endpoint with no in-`.bsl` references, so rename
    /// rewrites just its declaration.
    #[test]
    fn rename_query_name_rewrites_declaration() {
        let ws = TempWorkspace::new("rename_query");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { qty: int? }\n\
             shape W from Widget { qty }\n\
             query find() -> W[] { list Widget order (qty asc); }\n",
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
        let off = (snap.sources[fid].1.find("find()").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "list_widgets")
            .expect("query name is renameable");
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts, vec!["find".to_string()], "just the decl: {texts:?}");
    }

    /// (d) Renaming a field mapped to a *live* DB column also inserts `@was("old_col")`
    /// so the next generated migration renames the column (preserving data) — but only
    /// when the physical name actually changes and the column is captured in a snapshot.
    #[test]
    fn rename_field_inserts_was_for_live_column() {
        let ws = TempWorkspace::new("rename_was");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text?\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column barcode text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            !snap.diagnostics.iter().any(|d| d.code == "W0108"),
            "no drift expected: {:?}",
            snap.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "code")
            .expect("field is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // The rename + the data-preserving `@was` land on the field, and reparse clean.
        assert!(out.contains("code: text? @was(\"barcode\")"), "{out}");
        let reparsed = based_parser::parse_file(&out, FileId(0)).expect("applied source parses");
        let has_was = reparsed.decls.iter().any(|d| match d {
            Decl::Model(m) => m.members.iter().any(|mem| match mem {
                Member::Field(f) => {
                    f.name.node == "code"
                        && f.was.as_ref().map(|w| w.node.as_str()) == Some("barcode")
                }
                _ => false,
            }),
            _ => false,
        });
        assert!(has_was, "reparsed field carries @was(\"barcode\"): {out}");
    }

    /// A field with a `(column …)` override, an existing `@was`, or no captured
    /// migration snapshot gets no inserted `@was` (its physical name is decoupled,
    /// already named, or has no live column to preserve).
    #[test]
    fn rename_field_skips_was_when_not_data_preserving() {
        // Override decouples the physical name → renaming the field changes nothing in the DB.
        let ws = TempWorkspace::new("rename_was_skip");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text? (column \"upc\")\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column upc text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "code").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(!out.contains("@was"), "column override → no @was: {out}");

        // No captured migrations at all → nothing to preserve, no @was.
        let ws2 = TempWorkspace::new("rename_was_nomig");
        ws2.write("based.toml", "");
        ws2.write("schema.bsl", "Product {\n  barcode: text?\n}\n");
        let snap2 = compile_manifest(&ws2.root, &HashMap::new());
        let fid2 = snap2.file_id_of(&ws2.path("schema.bsl")).unwrap();
        let off2 = (snap2.sources[fid2].1.find("barcode:").unwrap() + 1) as u32;
        let out2 = apply_edits(
            &snap2,
            fid2,
            &snap2.rename_edits(fid2, off2, "code").unwrap()[&file_uri(&snap2, fid2)],
        );
        assert!(!out2.contains("@was"), "no migrations → no @was: {out2}");
    }

    /// Renaming a field already carrying `@was("orig")` keeps the *original* physical
    /// name as the was-source (the snapshot's column), so a rename chain still preserves
    /// data — it does not become `@was("<intermediate>")`.
    #[test]
    fn rename_field_keeps_original_was_across_a_chain() {
        let ws = TempWorkspace::new("rename_was_chain");
        ws.write("based.toml", "");
        // `barcode @was("upc")` is an uncaptured rename upc→barcode; the snapshot still
        // has `upc`. Renaming barcode→code must keep `@was("upc")`.
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text? @was(\"upc\")\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column upc text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "code").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(out.contains("code: text? @was(\"upc\")"), "{out}");
        assert!(
            !out.contains("@was(\"barcode\")"),
            "chain keeps orig: {out}"
        );
    }

    /// Renaming a model mapped to a live table inserts a leading `@was("old_table")`
    /// decorator, so the migration renames the table instead of drop+recreate.
    #[test]
    fn rename_model_inserts_was_for_live_table() {
        let ws = TempWorkspace::new("rename_model_was");
        ws.write("based.toml", "");
        ws.write("schema.bsl", "Product {\n  name: text\n}\n");
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column name text not_null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("Product").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "Item").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(
            out.starts_with("@was(\"product\")\nItem {"),
            "leading @was decorator + renamed model: {out}"
        );
        let reparsed = based_parser::parse_file(&out, FileId(0)).expect("applied source parses");
        assert!(reparsed
            .decls
            .iter()
            .any(|d| matches!(d, Decl::Model(m) if m.name.node == "Item")));
    }

    const ENUM_NAV_SCHEMA: &str = "\
enum Status { pending, paid, shipped }\n\
enum Grade { pending, top }\n\
Order { status: Status (default pending)  total: int }\n\
shape OrderRow from Order { status  total }\n\
query paid_orders() -> OrderRow[] { list Order where (status = paid) order (total); }\n\
query open_orders() -> OrderRow[] { list Order where (status in (paid, shipped)) order (total); }\n\
mutation mark(id: Id) -> OrderRow { update Order where (id = $id) { status = shipped } }\n";

    fn enum_nav_snapshot() -> (Snapshot, usize) {
        let ws = TempWorkspace::new("enum_nav");
        ws.write("based.toml", "");
        ws.write("schema.bsl", ENUM_NAV_SCHEMA);
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
        (snap, fid)
    }

    #[test]
    fn goto_def_on_a_variant_use_resolves_to_its_declaration() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // The declaration span of `paid` in `enum Status`.
        let decl = src.find("pending, paid, shipped").unwrap() + "pending, ".len();

        // `where (status = paid)` → the `paid` variant declaration.
        let use_off = (src.find("status = paid").unwrap() + "status = ".len()) as u32;
        let d = snap
            .definition_at(fid, use_off)
            .expect("variant use resolves");
        assert_eq!(
            d.start as usize,
            decl,
            "{}",
            &src[d.start as usize..d.end as usize]
        );

        // `default pending` → the `pending` variant declaration.
        let pend_decl = src.find("pending, paid").unwrap();
        let def_off = (src.find("default pending").unwrap() + "default ".len()) as u32;
        let d = snap
            .definition_at(fid, def_off)
            .expect("default variant resolves");
        assert_eq!(d.start as usize, pend_decl);

        // The write assign `status = shipped` → the `shipped` variant.
        let ship_decl = src.find("paid, shipped").unwrap() + "paid, ".len();
        let asg = (src.find("status = shipped").unwrap() + "status = ".len()) as u32;
        let d = snap
            .definition_at(fid, asg)
            .expect("assign variant resolves");
        assert_eq!(d.start as usize, ship_decl);
    }

    #[test]
    fn goto_def_on_a_variant_inside_an_in_list_resolves() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        let decl = src.find("pending, paid, shipped").unwrap() + "pending, ".len();
        // `where (status in (paid, shipped))` → the `paid` variant declaration.
        let use_off = (src.find("status in (paid").unwrap() + "status in (".len()) as u32;
        let d = snap
            .definition_at(fid, use_off)
            .expect("in-list variant use resolves");
        assert_eq!(
            d.start as usize,
            decl,
            "{}",
            &src[d.start as usize..d.end as usize]
        );
    }

    #[test]
    fn find_references_on_a_variant_are_enum_local() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // Cursor on the `pending` declaration in `enum Status`.
        let off = (src.find("pending, paid, shipped").unwrap() + 1) as u32;
        let refs = snap.references_at(fid, off, true);
        // The `Status.pending` decl + the `default pending` use — NOT `Grade.pending`.
        let texts: Vec<&str> = refs
            .iter()
            .map(|s| &src[s.start as usize..s.end as usize])
            .collect();
        assert!(texts.iter().all(|t| *t == "pending"), "{texts:?}");
        assert_eq!(
            refs.len(),
            2,
            "Status.pending decl + one use only: {texts:?}"
        );
        // None of the references is the Grade.pending declaration.
        let grade_pending = src.find("pending, top").unwrap();
        assert!(
            refs.iter().all(|s| s.start as usize != grade_pending),
            "Grade.pending must be untouched"
        );
    }

    #[test]
    fn rename_variant_rewrites_uses_and_leaves_same_named_variant_in_another_enum() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // Rename `Status.pending` (the declaration) to `queued`.
        let off = (src.find("pending, paid, shipped").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "queued")
            .expect("variant is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // Status's variant + its `default` use become `queued`.
        assert!(
            out.contains("enum Status { queued, paid, shipped }"),
            "{out}"
        );
        assert!(out.contains("default queued"), "{out}");
        // Grade's same-named `pending` variant is untouched.
        assert!(out.contains("enum Grade { pending, top }"), "{out}");
    }

    #[test]
    fn enum_type_ref_goto_def_and_hover_still_work() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // `status: Status` type reference → the `enum Status` declaration name.
        let ty_ref = (src.find("status: Status").unwrap() + "status: ".len()) as u32;
        let d = snap
            .definition_at(fid, ty_ref)
            .expect("enum type ref resolves");
        let enum_decl = src.find("Status {").unwrap();
        assert_eq!(d.start as usize, enum_decl);
        // Hover on the enum type reference names the enum.
        let h = snap.hover_at(fid, ty_ref).expect("enum hover");
        assert!(h.contains("enum Status"), "{h}");
        // Hover on a variant use names its enum.
        let vh = (src.find("status = paid").unwrap() + "status = ".len()) as u32;
        let h = snap.hover_at(fid, vh).expect("variant hover");
        assert!(h.contains("variant of enum `Status`"), "{h}");
    }

    /// The shared fixture for signature-binding navigation: a bound edge
    /// (`user -> author`), two `op col` bindings, and an unbound same-name param.
    fn binding_snapshot(tag: &str) -> (TempWorkspace, Snapshot) {
        let ws = TempWorkspace::new(tag);
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "User { name: text }\n\
             Post {\n  author: User\n  tags: json\n  created_at: timestamp\n}\n\
             query by_author(user -> author) -> Post[];\n\
             query search(tag: json has tags, since: timestamp > created_at) -> Post[];\n\
             query by_name(name) -> User[];\n",
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
        (ws, snap)
    }

    /// A signature binding's column/edge ident is a field reference like any other:
    /// go-to-def resolves it, find-references lists it, and renaming the field
    /// rewrites it (the NF-observed hole: a rename that skipped `has tags` silently
    /// broke the schema).
    #[test]
    fn binding_column_navigates_and_renames() {
        let (ws, snap) = binding_snapshot("binding_nav");
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = snap.sources[fid].1.clone();

        // Go-to-def on `tags` in `has tags` → the field declaration.
        let bind_off = (src.find("has tags").unwrap() + "has ".len()) as u32;
        let def = snap
            .definition_at(fid, bind_off)
            .expect("binding column resolves");
        assert_eq!(def.start as usize, src.find("tags: json").unwrap());

        // Go-to-def on `author` in `user -> author` → the relation declaration.
        let edge_off = (src.find("-> author").unwrap() + "-> ".len()) as u32;
        let edge_def = snap
            .definition_at(fid, edge_off)
            .expect("binding edge resolves");
        assert_eq!(edge_def.start as usize, src.find("author: User").unwrap());

        // Find-references from the field decl lists the binding site.
        let refs = snap.references_at(fid, def.start, false);
        assert!(
            refs.iter().any(|s| s.start == bind_off),
            "binding site listed: {refs:?}"
        );

        // Renaming the field rewrites its declaration AND the binding use.
        let changes = snap
            .rename_edits(fid, def.start, "labels")
            .expect("field is renameable");
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts.len(), 2, "decl + binding: {texts:?}");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("labels: json"), "{out}");
        assert!(out.contains("tag: json has labels"), "{out}");
    }

    /// Hovering a binding states the predicate it generates — on the column/edge
    /// ident (led by the field's signature), on the operator token itself, and on
    /// an unbound param (the derived same-name equality).
    #[test]
    fn binding_hover_states_generated_predicate() {
        let (ws, snap) = binding_snapshot("binding_hover");
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = snap.sources[fid].1.clone();

        // The bound column: field signature + generated predicate.
        let bind_off = (src.find("has tags").unwrap() + "has ".len()) as u32;
        let h = snap.hover_at(fid, bind_off).expect("column hover");
        assert!(h.contains("tags: json"), "{h}");
        assert!(h.contains("binds `tags has $tag`"), "{h}");
        assert!(h.contains("containment (array/json)"), "{h}");

        // The operator token alone still explains the binding.
        let has_off = src.find("has tags").unwrap() as u32;
        let h = snap.hover_at(fid, has_off).expect("operator hover");
        assert!(h.contains("binds `tags has $tag`"), "{h}");

        // An ordered op: the rendered predicate shows the column as left operand.
        let gt_off = src.find("> created_at").unwrap() as u32;
        let h = snap.hover_at(fid, gt_off).expect("ordered-op hover");
        assert!(h.contains("binds `created_at > $since`"), "{h}");
        assert!(h.contains("the column is the left operand"), "{h}");

        // An edge binding: relation signature + the FK equality it generates.
        let edge_off = (src.find("-> author").unwrap() + "-> ".len()) as u32;
        let h = snap.hover_at(fid, edge_off).expect("edge hover");
        assert!(h.contains("author: User"), "{h}");
        assert!(h.contains("binds `author = $user`"), "{h}");

        // An unbound param: the derived same-name equality is discoverable.
        let name_off = (src.find("by_name(name)").unwrap() + "by_name(".len()) as u32;
        let h = snap.hover_at(fid, name_off).expect("unbound param hover");
        assert!(h.contains("name: text"), "{h}");
        assert!(h.contains("binds `name = $name`"), "{h}");
    }

    /// The URI of file `fid` in a snapshot — a test convenience for indexing rename edits.
    fn file_uri(snap: &Snapshot, fid: usize) -> Url {
        Url::from_file_path(&snap.sources[fid].0).unwrap()
    }

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

    /// A raw-bodied query is inert editor surface: hover walks it without crashing,
    /// find-references lists a `${param}` use (at the raw block), and renaming the
    /// param rewrites only sites literally spelling the name — the opaque raw text
    /// is never corrupted.
    #[test]
    fn raw_query_body_is_inert_but_param_refs_resolve() {
        let ws = TempWorkspace::new("raw_query_body");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "User { name: text, total: int }\n\
             shape UserRow from User { name }\n\
             query heavy(min: int) -> UserRow[] {\n\
               raw`SELECT name FROM user WHERE total >= ${min}`;\n\
             }\n",
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

        // Hover across the whole raw line never panics.
        let line_start = src.find("raw`").unwrap() as u32;
        let line_end = src.find("`;").unwrap() as u32 + 1;
        for off in line_start..line_end {
            let _ = snap.hover_at(fid, off);
        }

        // The `${min}` use references the declared param (the raw block anchors it).
        let decl_off = (src.find("heavy(min").unwrap() + "heavy(".len()) as u32;
        let refs = snap.references_at(fid, decl_off, false);
        assert!(!refs.is_empty(), "raw `${{min}}` use should be listed");

        // Rename rewrites the decl only — the raw text spells `${min}` inside a
        // block whose span text is not `min`, so it is left alone (the miss is a
        // loud E0113 on recompile, not silent corruption).
        let changes = snap
            .rename_edits(fid, decl_off, "floor")
            .expect("param is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("query heavy(floor: int)"), "{out}");
        assert!(out.contains("${min}"), "raw text must be untouched: {out}");
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
