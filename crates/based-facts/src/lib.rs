//! based-facts — the engine-derived facts an editor should *show* (principle 8).
//!
//! "Show, don't write, for derived facts." A fact the compiler can derive — an
//! inverse relation's paired forward edge, a join-key index the engine will
//! create — must never be forced into source. Instead it is surfaced in the
//! editor. This crate computes those facts as span-anchored [`Fact`] values; the
//! LSP (or `based facts`) renders them as inlay hints / hover text.
//!
//! The computation is pure over the already-checked schema (plus the AST, only to
//! tell an *inferred* pairing from one the author wrote explicitly), so it is
//! golden-testable without an editor in the loop.

use based_ast::{Decl, Member, Span};
use based_sema::{CheckedSchema, MemberKind, RModel};

/// What kind of derived fact this is — the editor keys presentation off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    /// An inverse relation edge whose paired forward field the engine inferred:
    /// the author wrote `posts: Post[]`, not `posts: Post[] (Post.author)`.
    InferredInverse,
    /// A join-key index the engine will create even though no `@index` declares it
    /// (indexing.md, D15). Shown so the write cost is visible without living in
    /// source.
    InferredIndex,
}

impl FactKind {
    /// Stable short tag (used in `based facts` output + as an inlay-hint category).
    pub fn tag(self) -> &'static str {
        match self {
            FactKind::InferredInverse => "inverse",
            FactKind::InferredIndex => "index",
        }
    }
}

/// One engine-derived fact, anchored at the source span it annotates.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub span: Span,
    pub kind: FactKind,
    /// Terse label — what an editor shows inline (inlay hint).
    pub label: String,
    /// Fuller explanation — what an editor shows on hover, with the "why".
    pub detail: String,
}

/// Compute every derived fact worth surfacing for a checked schema.
///
/// `decls` is the same declaration set `check()` consumed; it is consulted only to
/// distinguish an *inferred* inverse pairing (author wrote `[]`) from an explicit
/// one (author wrote `(Model.field)`) — an explicit pairing is already in source,
/// so it is not a "show, don't write" fact. Output is sorted by span so the caller
/// (and goldens) see a stable order.
pub fn facts(schema: &CheckedSchema, decls: &[Decl]) -> Vec<Fact> {
    let mut out = Vec::new();

    for model in &schema.models {
        // Inferred inverse pairings: the forward edge this back-edge joins through.
        for mem in &model.members {
            if let MemberKind::Inverse { target, via } = &mem.kind {
                if inverse_was_inferred(decls, &model.name, &mem.name) {
                    out.push(Fact {
                        span: mem.span,
                        kind: FactKind::InferredInverse,
                        label: format!("<- {target} via {via}"),
                        detail: format!(
                            "inferred inverse: `{}.{}` pairs with forward edge `{target}.{via}` \
                             (engine-derived from the unique forward relation, relations.md)",
                            model.name, mem.name,
                        ),
                    });
                }
            }
        }

        // Inferred join-key indexes the DDL will emit (name + columns match `sql::ddl`).
        for idx in &model.inferred_indexes {
            let (name, cols) = inferred_index(model, &idx.columns);
            out.push(Fact {
                span: model.span,
                kind: FactKind::InferredIndex,
                label: format!("index {name} ({})", cols.join(", ")),
                detail: format!(
                    "inferred index `{name}` on ({}): join-key baseline for a traversed \
                     relation; the engine creates it so reads don't scan (indexing.md, D15). \
                     Add an explicit `@index` only to override it.",
                    cols.join(", "),
                ),
            });
        }
    }

    out.sort_by_key(|f| (f.span.file.0, f.span.start, f.span.end));
    out
}

/// True when the model's field declared an inverse edge with no explicit
/// `(Model.field)` pairing — i.e. the `via` in the IR was inferred, not written.
fn inverse_was_inferred(decls: &[Decl], model: &str, field: &str) -> bool {
    for d in decls {
        if let Decl::Model(m) = d {
            if m.name.node == model {
                return m.members.iter().any(|mem| {
                    matches!(mem, Member::Field(f) if f.name.node == field && f.inverse.is_none())
                });
            }
        }
    }
    false
}

/// Reproduce `sql::ddl`'s inferred-index name + physical column list so the shown
/// fact matches the generated DDL exactly: soft-delete column prepended
/// (predicate-leading — MariaDB has no partial indexes), name `inf_<table>_<cols>`
/// over the *field* names, display columns mapped to their physical names.
fn inferred_index(model: &RModel, columns: &[String]) -> (String, Vec<String>) {
    let mut fields = columns.to_vec();
    if let Some(sd) = &model.soft_delete {
        fields.insert(0, sd.field.clone());
    }
    let mut name = format!("inf_{}", model.table);
    for c in &fields {
        name.push('_');
        name.push_str(c);
    }
    let cols = fields.iter().map(|c| physical_col(model, c)).collect();
    (name, cols)
}

/// Field name -> physical column (scalar column / forward FK), matching `sql::ddl`.
fn physical_col(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
        _ => field.to_string(),
    }
}
