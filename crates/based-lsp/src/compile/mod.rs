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
    AggCall, Assign, AssignRhs, BaseType, Clause, Decl, EnumDecl, Field, FileId, GeneratedField,
    Ident, Member, Model, Modifier, Mutation, NamedFilter, Op, Param, ParamBinding, ParamRef,
    Predicate, Primitive, Query, QueryBody, RawPart, RawSql, ScopeDecl, Shape, ShapeExpr,
    ShapeField, ShapeValue, Span, TypeExpr, Value, VariantValue, WriteStmt,
};
use based_diagnostics::Diagnostic;
use based_facts::{Fact, FactKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, FoldingRange, FoldingRangeKind, InlayHint,
    InlayHintKind, InlayHintLabel, InlayHintLabelPart, InlayHintTooltip, Location, Position, Range,
    SelectionRange, SemanticToken, SymbolInformation, SymbolKind, TextEdit, Url,
};

use crate::sqltok;

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
    /// The project's compile-target dialect, when it has a manifest. Drives the
    /// per-dialect SQL highlighting of `raw`…`` interiors; a loose file with no
    /// manifest highlights against the documented default.
    pub dialect: Option<based_codegen::Dialect>,
}

impl Snapshot {
    /// `FileId` index for a path, matched by canonicalized path.
    pub fn file_id_of(&self, path: &Path) -> Option<usize> {
        let want = canon(path);
        self.sources.iter().position(|(p, _)| canon(p) == want)
    }


    /// The source text a span covers, within its owning file.
    pub(super) fn span_text(&self, span: Span) -> Option<&str> {
        let (_, src) = self.sources.get(span.file.0 as usize)?;
        src.get(span.start as usize..span.end as usize)
    }


    /// The owning file's URI for a file id — the target document of a quick-fix edit.
    pub fn file_uri(&self, fid: usize) -> Option<Url> {
        let (path, _) = self.sources.get(fid)?;
        Url::from_file_path(path).ok()
    }

}

mod navigation;
mod rename;
mod folding;
mod format;
mod inlay;
mod semantic;
mod symbols;
mod resolve;
mod enums;
mod bindings;
mod hover;
mod completions;
mod collect;
mod build;
mod drift;
mod line_index;
#[cfg(test)]
mod testsupport;

pub use build::{compile_manifest, compile_loose, find_manifest_root, canon};
pub use line_index::LineIndex;
pub(crate) use line_index::{span_range, word_extent, line_start};
pub(crate) use drift::{latest_snapshot, drift_diagnostics};
pub(crate) use collect::*;
