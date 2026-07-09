//! based-facts — the engine-derived facts an editor should *show*.
//!
//! "Show, don't write, for derived facts." A fact the compiler can derive — an
//! inverse relation's paired forward edge, a join-key index the engine will
//! create — must never be forced into source. Instead it is surfaced in the editor.
//! This crate computes those facts as span-anchored [`Fact`] values; the LSP (or
//! `based facts`) renders them as inlay hints / hover text.
//!
//! The computation is pure over the already-checked schema (plus the AST, only to
//! tell an *inferred* pairing from one the author wrote explicitly), so it is
//! golden-testable without an editor in the loop.

use based_ast::{Decl, Member, Primitive, Scoped, Span, Verb};
use based_sema::{CheckedSchema, CtxField, CtxReq, MemberKind, RModel, RQuery, RScope};

/// What kind of derived fact this is — the editor keys presentation off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    /// An inverse relation edge whose paired forward field the engine inferred:
    /// the author wrote `posts: Post[]`, not `posts: Post[] (Post.author)`.
    InferredInverse,
    /// A join-key index the engine will create even though no `@index` declares it.
    /// Shown so the write cost is visible without living in source.
    InferredIndex,
    /// The `$ctx` fields a callable requires — the request context the generated
    /// client sends. Each field's type is fixed by the scope decl or the column it
    /// binds to; a field used only in a hand-written `where` is typed by that use.
    CtxRequirement,
    /// A query's resolved shape — verb (`get`/`list`), target model, cardinality,
    /// pagination — inferred from the return shape + cardinality. None of it is
    /// written in the signature.
    ResolvedQuery,
    /// A named scope contract: its filter terms, the models it governs, and how
    /// callables opt in. Surfaced on the `scope` decl and on every `@scope`/`scoped`
    /// reference so hovering either explains the contract.
    Scope,
}

impl FactKind {
    /// Stable short tag (used in `based facts` output + as an inlay-hint category).
    pub fn tag(self) -> &'static str {
        match self {
            FactKind::InferredInverse => "inverse",
            FactKind::InferredIndex => "index",
            FactKind::CtxRequirement => "ctx",
            FactKind::ResolvedQuery => "query",
            FactKind::Scope => "scope",
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
    /// A related declaration this fact's inlay can point at (command-click). For an
    /// inferred inverse it is the paired forward edge's span — so the `via order`
    /// hint on `Order.items` jumps to `OrderItem.order`. `None` for facts with
    /// nowhere to follow.
    pub nav: Option<Span>,
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
                    // Point the inlay's command-click at the paired forward edge.
                    let nav = schema
                        .model(target)
                        .and_then(|m| m.member(via))
                        .map(|f| f.span);
                    out.push(Fact {
                        span: mem.span,
                        kind: FactKind::InferredInverse,
                        // The forward edge the engine paired through, qualified by its
                        // model: `via OrderItem.order` reads unambiguously even when the
                        // field name (`order`) echoes a model name.
                        label: format!("via {target}.{via}"),
                        detail: format!(
                            "inferred inverse: `{}.{}` pairs with the forward edge \
                             `{target}.{via}`, which the engine derived from the one \
                             forward relation between these models.",
                            model.name, mem.name,
                        ),
                        nav,
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
                    "inferred index `{name}` on ({}): a join-key baseline for a traversed \
                     relation; the engine creates it so reads don't scan. Add an explicit \
                     `@index` only to override it.",
                    cols.join(", "),
                ),
                nav: None,
            });
        }
    }

    // Per-query facts: the resolved shape and the `$ctx` requirement bag — neither
    // is written in the signature.
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

    // Scope-contract facts: one per reference site (the decl + every `@scope`/`scoped`),
    // so hovering any of them explains the standing filter.
    scope_facts(schema, decls, &mut out);

    out.sort_by_key(|f| (f.span.file.0, f.span.start, f.span.end));
    out
}

/// Surface each named scope as a hover fact at every site that names it: the `scope` decl
/// itself and every `@scope Name` / `scoped Name` reference. The detail names the scope: its
/// filter terms and the models it governs.
fn scope_facts(schema: &CheckedSchema, decls: &[Decl], out: &mut Vec<Fact>) {
    for scope in &schema.scopes {
        let detail = scope_detail(schema, scope);
        let label = scope_label(scope);
        let mut push = |span: Span| {
            out.push(Fact {
                span,
                kind: FactKind::Scope,
                label: label.clone(),
                detail: detail.clone(),
                nav: None,
            });
        };
        for d in decls {
            match d {
                // The `scope Name (…)` decl — anchor on its name.
                Decl::Scope(s) if s.name.node == scope.name => push(s.name.span),
                // `@scope Name[, Name]*` on a model — one ref per matching name.
                Decl::Model(m) => {
                    for r in &m.scopes {
                        for id in &r.names {
                            if id.node == scope.name {
                                push(id.span);
                            }
                        }
                    }
                }
                // `scoped Name[, Name]*` on a callable.
                Decl::Query(q) => scoped_refs(q.scoped.as_ref(), &scope.name, &mut push),
                Decl::Mutation(mu) => scoped_refs(mu.scoped.as_ref(), &scope.name, &mut push),
                _ => {}
            }
        }
    }
}

/// Push a hover anchor for each `scoped Name` reference matching `name`.
fn scoped_refs(scoped: Option<&Scoped>, name: &str, push: &mut impl FnMut(Span)) {
    if let Some(s) = scoped {
        for id in &s.names {
            if id.node == name {
                push(id.span);
            }
        }
    }
}

/// The terse label for a scope — its name + filter — for the `based facts` listing.
fn scope_label(scope: &RScope) -> String {
    format!("scope {} = {}", scope.name, scope_filter(scope))
}

/// The `col = $ctx.field [and …]` filter a scope injects.
fn scope_filter(scope: &RScope) -> String {
    scope
        .terms
        .iter()
        .map(|t| format!("{} = $ctx.{}", t.column, t.ctx_field))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Name a scope for hover: its filter predicate and the models it governs.
fn scope_detail(schema: &CheckedSchema, scope: &RScope) -> String {
    let filter = scope_filter(scope);
    let governed: Vec<&str> = schema
        .models
        .iter()
        .filter(|m| {
            m.scope_alts
                .iter()
                .any(|alt| alt.iter().any(|n| n == &scope.name))
        })
        .map(|m| m.name.as_str())
        .collect();
    let governs = if governed.is_empty() {
        "governs no models".to_string()
    } else {
        format!("governs {}", governed.join(", "))
    };
    format!("scope `{}`: filter `{filter}`; {governs}", scope.name)
}

/// The `$ctx` bag a callable requires, as one aggregate fact anchored at its
/// declaration — `None` when it needs no context. Each field's type is inferred
/// from the column its use compares against.
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
        detail: format!("request context: this {kind} requires `$ctx` [{bag}]"),
        nav: None,
    })
}

/// A `$ctx` field's inferred type, rendered like the sema conformance summary: a
/// primitive verbatim, a relation as `-> Model`.
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
/// which appears in the signature.
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
            "resolved query: reads `{}` as a `{verb}` returning {}, inferred from the \
             return shape and cardinality{}.",
            q.target,
            if q.many { "many rows" } else { "one row" },
            if q.paginated {
                "; keyset-paginated"
            } else {
                ""
            },
        ),
        nav: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use based_ast::FileId;

    /// Parse + check a snippet into a schema + its decls, asserting it is clean.
    fn build(src: &str) -> (CheckedSchema, Vec<Decl>) {
        let sf = based_parser::parse_file(src, FileId(0)).expect("parse");
        let (schema, diags) = based_sema::check(&sf.decls);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == based_diagnostics::Severity::Error)
            .map(|d| d.code)
            .collect();
        assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
        (schema, sf.decls)
    }

    // A small scoped schema: one scope, one governed model, one scoped query + mutation.
    const SCOPED: &str = "
        scope Tenant (org: Org = $ctx.org)
        Org { name: text }
        @scope Tenant
        Widget { org: Org  name: text }
        shape WidgetCard from Widget { name }
        query widgets() -> WidgetCard[] scoped Tenant { list Widget; }
        mutation add_widget(name: text) -> WidgetCard scoped Tenant {
          create Widget { name = $name };
        }
    ";

    #[test]
    fn scope_hover_facts_cover_decl_and_every_reference() {
        let (schema, decls) = build(SCOPED);
        let all = facts(&schema, &decls);
        let scope_facts: Vec<&Fact> = all.iter().filter(|f| f.kind == FactKind::Scope).collect();
        // The decl, the `@scope Tenant`, and two `scoped Tenant` references = 4 anchors.
        assert_eq!(scope_facts.len(), 4, "{scope_facts:#?}");
        let d = &scope_facts[0].detail;
        // Names the scope: its filter predicate and the model it governs.
        assert!(d.contains("scope `Tenant`"), "{d}");
        assert!(d.contains("org = $ctx.org"), "{d}");
        assert!(d.contains("governs Widget"), "{d}");
    }

    /// Regression guard: no editor-facing hover/inlay string may leak an internal
    /// decision-record ref (`D<n>`), a principle ref (`P<n>`/"principle"), or a spec-doc
    /// filename. The stable diagnostic codes (`E01xx`/`W01xx`) live elsewhere.
    #[test]
    fn no_editor_string_leaks_a_decision_or_principle_ref() {
        let (schema, decls) = build(SCOPED);
        let leak = |s: &str| -> bool {
            let low = s.to_lowercase();
            low.contains("principle")
                || low.contains(".md")
                || s.split(|c: char| !c.is_alphanumeric()).any(|w| {
                    let mut ch = w.chars();
                    matches!(ch.next(), Some('D') | Some('P'))
                        && ch.clone().count() >= 1
                        && ch.all(|c| c.is_ascii_digit())
                })
        };
        for f in facts(&schema, &decls) {
            assert!(!leak(&f.label), "label leaks a ref: {:?}", f.label);
            assert!(!leak(&f.detail), "detail leaks a ref: {:?}", f.detail);
        }
    }
}
