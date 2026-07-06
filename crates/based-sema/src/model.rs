//! Model resolution: AST `Model` -> `RModel`.
//!
//! Two phases, because a relation's target and an inverse's forward edge may be
//! declared in any file (D6): [`skeleton`] records every model's fields without
//! cross-checking, then [`validate`] resolves relation targets, inverses,
//! decorators, indexes, and sorts once all skeletons exist.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve;

/// Build an unvalidated `RModel` from one AST model: implicit `id`, columns, and
/// relations classified forward vs. inverse. No cross-model checks yet.
pub fn skeleton(m: &Model, sink: &mut Sink) -> RModel {
    let mut members: Vec<RMember> = Vec::new();
    let mut seen: HashMap<String, Span> = HashMap::new();

    for mem in &m.members {
        let Member::Field(f) = mem else { continue };
        if seen.contains_key(&f.name.node) {
            sink.error(
                code::DUP_FIELD,
                f.name.span,
                format!(
                    "duplicate field `{}` in model `{}`",
                    f.name.node, m.name.node
                ),
            );
            continue;
        }
        seen.insert(f.name.node.clone(), f.name.span);
        // Field `(default now())` functions are validated against the closed set.
        for m in &f.modifiers {
            if let Modifier::Default(dv) = m {
                resolve::check_default(dv, sink);
            }
        }
        members.push(RMember {
            name: f.name.node.clone(),
            span: f.name.span,
            kind: classify(f),
        });
    }

    // Implicit `id: Id` unless the model declares its own key (D1/D2).
    if !seen.contains_key("id") {
        members.insert(
            0,
            RMember {
                name: "id".to_string(),
                span: m.name.span,
                kind: MemberKind::Scalar {
                    ty: Primitive::Id,
                    optional: false,
                    many: false,
                    column: "id".to_string(),
                    unique: false, // PK, expressed as PRIMARY KEY not a UNIQUE constraint
                    default: None, // engine-generated on insert (D1), no SQL default
                },
            },
        );
    }

    let table = table_name(m);
    RModel {
        name: m.name.node.clone(),
        span: m.span,
        table,
        members,
        soft_delete: None,
        sort: Vec::new(),
        scope: None,
        created: None,
        updated: None,
        indexes: Vec::new(),
        inferred_indexes: Vec::new(),
        unique_cols: Vec::new(),
    }
}

/// Physical table name: `@table("…")` override else `snake_case(Name)`.
fn table_name(m: &Model) -> String {
    for d in &m.decorators {
        if d.name.node == "table" {
            if let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() {
                return s.clone();
            }
        }
    }
    snake_case(&m.name.node)
}

/// Classify a field as a scalar column, a forward relation (FK here), or an
/// inverse edge (`[]`, or an explicit `(Model.field)` pairing).
fn classify(f: &Field) -> MemberKind {
    match &f.ty.base {
        BaseType::Primitive(p) => MemberKind::Scalar {
            ty: *p,
            optional: f.ty.optional,
            many: f.ty.many,
            column: column_override(f).unwrap_or_else(|| f.name.node.clone()),
            unique: f.modifiers.iter().any(|m| matches!(m, Modifier::Unique)),
            default: f.modifiers.iter().find_map(|m| match m {
                Modifier::Default(dv) => Some(dv.clone()),
                _ => None,
            }),
        },
        BaseType::Model(target) => {
            // A to-many model edge, or one carrying an explicit inverse ref, is a
            // back edge; its FK lives on the target. `via` is filled in `validate`
            // (explicit ref, or inferred from the target's forward edges).
            if f.ty.many || f.inverse.is_some() {
                MemberKind::Inverse {
                    target: target.node.clone(),
                    via: f
                        .inverse
                        .as_ref()
                        .map(|iv| iv.field.node.clone())
                        .unwrap_or_default(),
                }
            } else {
                let fk_col = column_override(f).unwrap_or_else(|| format!("{}_id", f.name.node));
                MemberKind::Forward {
                    target: target.node.clone(),
                    optional: f.ty.optional,
                    fk_col,
                    custom_join: f.relation_on.is_some(),
                }
            }
        }
    }
}

fn column_override(f: &Field) -> Option<String> {
    f.modifiers.iter().find_map(|m| match m {
        Modifier::Column(c) => Some(c.clone()),
        _ => None,
    })
}

/// Resolve the structural facts that only touch *this* model plus name lookups:
/// relation targets, inverse pairings, decorator roles, indexes, and uniqueness.
/// Expression resolution (scope/sort paths, which traverse *other* models) is done
/// afterwards in [`resolve_exprs`], once every model is fully built — otherwise the
/// read-only path checker would alias this `&mut` pass.
pub fn validate(
    ast: &Model,
    mi: usize,
    models: &mut [RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    validate_relations(mi, models, index, sink);
    validate_indexes(ast, mi, models, sink);
    validate_decorators(ast, mi, models, sink);
    compute_unique(ast, &mut models[mi]);
}

fn validate_relations(
    mi: usize,
    models: &mut [RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    // Collect fixups without holding an aliasing borrow of `models`.
    let mut infer: Vec<(usize, String)> = Vec::new(); // (member idx, inferred via)
    {
        let m = &models[mi];
        for (i, mem) in m.members.iter().enumerate() {
            match &mem.kind {
                MemberKind::Forward { target, .. } => {
                    if !index.contains_key(target) {
                        sink.error(
                            code::UNKNOWN_MODEL,
                            mem.span,
                            format!("relation `{}` names unknown model `{target}`", mem.name),
                        );
                    }
                }
                MemberKind::Inverse { target, via } => {
                    let Some(&ti) = index.get(target) else {
                        sink.error(
                            code::UNKNOWN_MODEL,
                            mem.span,
                            format!("relation `{}` names unknown model `{target}`", mem.name),
                        );
                        continue;
                    };
                    if via.is_empty() {
                        // Infer: the unique forward edge on `target` back to us.
                        match infer_inverse(&models[ti], &m.name) {
                            Ok(field) => infer.push((i, field)),
                            Err(msg) => sink.error(code::INVERSE_INFER, mem.span, msg),
                        }
                    } else {
                        // Explicit `(Model.field)`: must be a forward edge to us.
                        check_inverse_ref(&models[ti], via, &m.name, mem, sink);
                    }
                }
                MemberKind::Scalar { .. } => {}
            }
        }
    }
    for (i, field) in infer {
        if let MemberKind::Inverse { via, .. } = &mut models[mi].members[i].kind {
            *via = field;
        }
    }
}

/// The unique forward edge on `target` whose type is `me`; error text otherwise.
fn infer_inverse(target: &RModel, me: &str) -> Result<String, String> {
    let candidates: Vec<&str> = target
        .members
        .iter()
        .filter_map(|mem| match &mem.kind {
            MemberKind::Forward { target: t, .. } if t == me => Some(mem.name.as_str()),
            _ => None,
        })
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(format!(
            "no forward edge from `{}` back to `{me}` to invert; add one or an explicit `({}.field)`",
            target.name, target.name
        )),
        many => Err(format!(
            "ambiguous inverse: `{}` has {} edges to `{me}` ({}); disambiguate with `({}.field)`",
            target.name,
            many.len(),
            many.join(", "),
            target.name
        )),
    }
}

fn check_inverse_ref(target: &RModel, via: &str, me: &str, mem: &RMember, sink: &mut Sink) {
    match target.member(via).map(|m| &m.kind) {
        Some(MemberKind::Forward { target: t, .. }) if t == me => {}
        Some(_) => sink.error(
            code::INVERSE_REF,
            mem.span,
            format!("`{}.{via}` is not a forward edge to `{me}`", target.name),
        ),
        None => sink.error(
            code::INVERSE_REF,
            mem.span,
            format!("`{}` has no field `{via}`", target.name),
        ),
    }
}

fn validate_indexes(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mut indexes = Vec::new();
    for mem in &ast.members {
        let Member::Index(idx) = mem else { continue };
        for col in &idx.columns {
            if models[mi].member(&col.node).is_none() {
                sink.error(
                    code::INDEX_COLUMN,
                    col.span,
                    format!("index names unknown column `{}`", col.node),
                );
            }
        }
        indexes.push(RIndex {
            columns: idx.columns.iter().map(|c| c.node.clone()).collect(),
            unique: idx.unique,
        });
    }
    models[mi].indexes = indexes;
}

fn validate_decorators(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mut sort_terms: Vec<SortTerm> = Vec::new();
    for d in &ast.decorators {
        match d.name.node.as_str() {
            "soft_delete" => {
                if let Some(field) = deco_field(d) {
                    resolve_soft_delete(field, mi, models, sink);
                }
            }
            "created" | "updated" => {
                if let Some(field) = deco_field(d) {
                    resolve_managed_ts(&d.name.node, field, mi, models, sink);
                }
            }
            // Scope/sort *paths* traverse other models — resolved in `resolve_exprs`.
            "scope" => {
                if let Some(DecoArg::Pred(p)) = d.args.first() {
                    models[mi].scope = Some(p.clone());
                }
            }
            "sort" => {
                for a in &d.args {
                    if let DecoArg::Sort(t) = a {
                        sort_terms.push(t.clone());
                    }
                }
            }
            "table" => {} // consumed for the table name in `skeleton`
            other => sink.warn(
                code::UNKNOWN_DECORATOR,
                d.name.span,
                format!("unknown decorator `@{other}` (ignored)"),
            ),
        }
    }
    models[mi].sort = sort_terms;
}

/// A decorator's target field, from a bare ident or a single-segment path.
fn deco_field(d: &Decorator) -> Option<&Ident> {
    match d.args.first()? {
        DecoArg::Ident(id) => Some(id),
        DecoArg::Path(p) if p.segments.len() == 1 => Some(&p.segments[0]),
        _ => None,
    }
}

fn resolve_soft_delete(field: &Ident, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mode = match models[mi].member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Scalar {
            ty: Primitive::Timestamp | Primitive::Date,
            optional: true,
            many: false,
            ..
        }) => Some(SoftMode::Timestamp),
        Some(MemberKind::Scalar {
            ty: Primitive::Bool,
            many: false,
            ..
        }) => Some(SoftMode::Bool),
        Some(_) => {
            sink.error_note(
                code::SOFT_DELETE_TYPE,
                field.span,
                format!("`{}` cannot back @soft_delete", field.node),
                "covered subset: nullable `timestamp`/`date`, or `bool` — else drop to a raw override",
            );
            None
        }
        None => {
            sink.error(
                code::DECO_TARGET,
                field.span,
                format!("@soft_delete names unknown field `{}`", field.node),
            );
            None
        }
    };
    if let Some(mode) = mode {
        models[mi].soft_delete = Some(SoftDelete {
            field: field.node.clone(),
            mode,
        });
    }
}

fn resolve_managed_ts(
    deco: &str,
    field: &Ident,
    mi: usize,
    models: &mut [RModel],
    sink: &mut Sink,
) {
    match models[mi].member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Scalar {
            ty: Primitive::Timestamp | Primitive::Date,
            ..
        }) => {
            if deco == "created" {
                models[mi].created = Some(field.node.clone());
            } else {
                models[mi].updated = Some(field.node.clone());
            }
        }
        Some(_) => sink.error(
            code::DECO_TARGET,
            field.span,
            format!("@{deco} field `{}` must be a timestamp/date", field.node),
        ),
        None => sink.error(
            code::DECO_TARGET,
            field.span,
            format!("@{deco} names unknown field `{}`", field.node),
        ),
    }
}

/// Field names that are individually unique: `id`, `(unique)` scalars, and
/// single-column unique indexes. (Composite unique indexes make no *single*
/// column unique, so they do not count here.)
fn compute_unique(ast: &Model, m: &mut RModel) {
    let mut unique = vec!["id".to_string()];
    for mem in &ast.members {
        match mem {
            Member::Field(f) if f.modifiers.iter().any(|x| matches!(x, Modifier::Unique)) => {
                unique.push(f.name.node.clone());
            }
            Member::Index(idx) if idx.unique && idx.columns.len() == 1 => {
                unique.push(idx.columns[0].node.clone());
            }
            _ => {}
        }
    }
    unique.dedup();
    m.unique_cols = unique;
}

/// Read-only pass over one model's expression-valued decorators/fields: `@scope`
/// predicates, model `@sort` terms, and relation-field `@sort` terms. Run after
/// every model is built, so path traversal into other models is safe.
pub fn resolve_exprs(ast: &Model, cx: &resolve::Cx, sink: &mut Sink) {
    let Some(mi) = cx.find(&ast.name.node) else {
        return;
    };
    for d in &ast.decorators {
        match d.name.node.as_str() {
            // Scope predicates see only `$ctx` (no query params, auth.md), and are
            // restricted to a conjunction of `col = $ctx.field` equalities (D32) so
            // scope is injectable everywhere and auto-settable on `create`.
            "scope" => {
                if let Some(DecoArg::Pred(p)) = d.args.first() {
                    resolve::check_predicate(p, Some(mi), cx, &[], sink);
                    resolve::check_scope_form(p, d.name.span, sink);
                }
            }
            "sort" => {
                for a in &d.args {
                    if let DecoArg::Sort(t) = a {
                        resolve::check_sort_term(t, mi, cx, sink);
                    }
                }
            }
            _ => {}
        }
    }
    // Custom `on:` joins span two tables (this model + the relation target), so
    // resolve them here in the read pass where other models are reachable (D17).
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(pred) = &f.relation_on else { continue };
        match &f.ty.base {
            // A to-one forward relation — the only edge that owns a join. `on:` on a
            // scalar, an optional is fine; a `[]` / explicit-inverse edge owns no FK.
            BaseType::Model(target) if !f.ty.many && f.inverse.is_none() => {
                if let Some(fi) = cx.find(&target.node) {
                    resolve::check_relation_on(pred, mi, fi, cx, sink);
                }
            }
            _ => sink.error(
                code::JOIN_FORM,
                f.name.span,
                format!(
                    "`on:` custom join applies only to a to-one relation, not `{}`",
                    f.name.node
                ),
            ),
        }
    }

    // Relation `@sort` sorts the *target* rows; resolve terms against the target.
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(terms) = &f.sort else { continue };
        let ctx = match cx.model(mi).member(&f.name.node).map(|m| &m.kind) {
            Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                cx.find(target)
            }
            _ => Some(mi),
        };
        if let Some(ci) = ctx {
            for t in terms {
                resolve::check_sort_term(t, ci, cx, sink);
            }
        }
    }
}
