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

use based_ast::{Decl, Member, Primitive, Span, Verb};
use based_sema::{CheckedSchema, CtxField, CtxReq, MemberKind, RModel, RQuery};

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
    /// The `$ctx` fields a callable silently requires — inferred per-callable from
    /// the columns each use compares against (D4/D5). Nothing in source declares the
    /// request contract; the generated client sends exactly these.
    CtxRequirement,
    /// A query's resolved shape — verb (`get`/`list`), target model, cardinality,
    /// pagination — inferred from the return shape + cardinality (queries.md). None
    /// of it is written in the signature.
    ResolvedQuery,
}

impl FactKind {
    /// Stable short tag (used in `based facts` output + as an inlay-hint category).
    pub fn tag(self) -> &'static str {
        match self {
            FactKind::InferredInverse => "inverse",
            FactKind::InferredIndex => "index",
            FactKind::CtxRequirement => "ctx",
            FactKind::ResolvedQuery => "query",
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

    // Per-query facts: the resolved shape (queries.md inferences) and the `$ctx`
    // requirement bag — neither is written in the signature.
    for q in &schema.queries {
        out.push(resolved_query_fact(q));
        if let Some(fact) = ctx_fact(q.span, &q.ctx_requires, "query") {
            out.push(fact);
        }
    }
    // Mutations carry no inferred shape (their write model is explicit), but they do
    // require context the same way.
    for m in &schema.mutations {
        if let Some(fact) = ctx_fact(m.span, &m.ctx_requires, "mutation") {
            out.push(fact);
        }
    }

    out.sort_by_key(|f| (f.span.file.0, f.span.start, f.span.end));
    out
}

/// The `$ctx` bag a callable requires, as one aggregate fact anchored at its
/// declaration — `None` when it needs no context. The bag is inference-derived
/// (D4/D5): each field's type comes from the column its use compares against, and
/// the generated client sends exactly this set.
fn ctx_fact(span: Span, reqs: &[CtxReq], kind: &str) -> Option<Fact> {
    if reqs.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = reqs
        .iter()
        .map(|r| format!("{}: {}", r.field, ctx_ty(&r.ty)))
        .collect();
    parts.sort();
    let bag = parts.join(", ");
    Some(Fact {
        span,
        kind: FactKind::CtxRequirement,
        label: format!("requires [{bag}]"),
        detail: format!(
            "request context: this {kind} requires `$ctx` fields [{bag}], inferred \
             per-callable from the columns they compare against (D4/D5). Nothing in \
             source declares them — the generated client sends exactly these.",
        ),
    })
}

/// A `$ctx` field's inferred type, rendered like the sema conformance summary: a
/// primitive verbatim, a relation as `-> Model` (the caller supplies its key, D1).
fn ctx_ty(ty: &CtxField) -> String {
    match ty {
        CtxField::Scalar(p) => prim(*p).to_string(),
        CtxField::Relation(m) => format!("-> {m}"),
    }
}

/// Primitive -> its DSL spelling (matches the sema conformance summary; `Id` keeps
/// its casing, the rest are lowercase).
fn prim(p: Primitive) -> &'static str {
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

/// The resolved shape of a query: verb + target + cardinality + pagination, none of
/// which appears in the signature (queries.md infers them from the return shape).
fn resolved_query_fact(q: &RQuery) -> Fact {
    let verb = match q.verb {
        Verb::Get => "get",
        Verb::List => "list",
    };
    let card = if q.many { "[]" } else { "" };
    let mut label = format!("{verb} {}{card}", q.target);
    if q.paginated {
        label.push_str(" paginated");
    }
    Fact {
        span: q.span,
        kind: FactKind::ResolvedQuery,
        label,
        detail: format!(
            "resolved query: reads `{}` as a `{verb}` returning {} (inferred from the \
             return shape and cardinality, queries.md){}.",
            q.target,
            if q.many { "many rows" } else { "one row" },
            if q.paginated {
                "; keyset-paginated"
            } else {
                ""
            },
        ),
    }
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
