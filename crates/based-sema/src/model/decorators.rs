use super::*;

pub(crate) fn validate_indexes(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
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
        if let Some(m) = &idx.method {
            if index_method_targets(&m.node).is_none() {
                let known = INDEX_METHODS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ");
                sink.error_note(
                    code::INDEX_METHOD,
                    m.span,
                    format!("unknown index access method `{}`", m.node),
                    format!("known methods: {known}"),
                );
            }
        }
        if let Some(spec) = &idx.raw {
            check_raw_spec(spec, "index", sink);
        }
        indexes.push(RIndex {
            columns: idx.columns.iter().map(|c| c.node.clone()).collect(),
            unique: idx.unique,
            method: idx.method.as_ref().map(|m| m.node.clone()),
            raw: idx.raw.clone(),
            span: idx.span,
        });
    }
    models[mi].indexes = indexes;
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        if let BaseType::Raw(spec) = &f.ty.base {
            check_raw_spec(spec, "type", sink);
        }
    }
}

/// A `raw(…)` body must carry a non-empty literal for every dialect it names — an empty
/// one would emit a syntactically broken column type or index.
pub(crate) fn check_raw_spec(spec: &RawSpec, what: &str, sink: &mut Sink) {
    for lit in spec.literals() {
        if lit.node.trim().is_empty() {
            sink.error(
                code::RAW_EMPTY,
                lit.span,
                format!("an empty `raw(…)` {what} body"),
            );
        }
    }
    if let RawSpecBody::PerDialect(entries) = &spec.body {
        for e in entries {
            if !DIALECTS.contains(&e.dialect.node.as_str()) {
                sink.error_note(
                    code::RAW_TYPE_DIALECT,
                    e.dialect.span,
                    format!("unknown dialect `{}` in a `raw({{…}})` map", e.dialect.node),
                    format!("compile targets: {}", DIALECTS.join(", ")),
                );
            }
        }
    }
}

pub(crate) fn validate_decorators(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
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
            // `@scope Name` is parsed into `Model.scopes`, not `decorators`, and is
            // resolved by the scope pass — it never reaches here.
            "sort" => {
                for a in &d.args {
                    if let Some(t) = deco_sort_term(a) {
                        sort_terms.push(t);
                    }
                }
            }
            "table" => {} // consumed for the table name in `skeleton`
            "was" => {}   // model rename directive — validated in `validate_was`
            "no_id" => {} // keyless opt-out — consumed + validated in `skeleton`
            "no_fk" => {} // whole-table FK opt-out — consumed in `skeleton`, divergence in the manifest pass
            "key" => {}   // natural-key nomination — consumed + validated in `skeleton`
            other => sink.warn(
                code::UNKNOWN_DECORATOR,
                d.name.span,
                format!("unknown decorator `@{other}` (ignored)"),
            ),
        }
    }
    models[mi].sort = sort_terms;
}

/// A `@sort` decorator argument as a sort term. A bare path carries no direction
/// token, so the argument scan can't classify it as a sort — it arrives as an
/// `Ident`/`Path` arg and defaults to ascending here (grammar: the direction is
/// optional, bare = `asc`).
pub(crate) fn deco_sort_term(a: &DecoArg) -> Option<SortTerm> {
    match a {
        DecoArg::Sort(t) => Some(t.clone()),
        DecoArg::Ident(id) => Some(SortTerm {
            path: Path {
                segments: vec![id.clone()],
            },
            dir: SortDir::Asc,
            nulls: None,
        }),
        DecoArg::Path(p) => Some(SortTerm {
            path: p.clone(),
            dir: SortDir::Asc,
            nulls: None,
        }),
        DecoArg::Pred(_) | DecoArg::Lit(_) => None,
    }
}

/// A decorator's target field, from a bare ident or a single-segment path.
pub(crate) fn deco_field(d: &Decorator) -> Option<&Ident> {
    match d.args.first()? {
        DecoArg::Ident(id) => Some(id),
        DecoArg::Path(p) if p.segments.len() == 1 => Some(&p.segments[0]),
        _ => None,
    }
}

pub(crate) fn resolve_soft_delete(
    field: &Ident,
    mi: usize,
    models: &mut [RModel],
    sink: &mut Sink,
) {
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

pub(crate) fn resolve_managed_ts(
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
pub(crate) fn compute_unique(ast: &Model, m: &mut RModel) {
    // The single primary-key column is unique: `id`, or a single-column `@key(field)`. A
    // composite `@key` makes no *single* column unique (only the tuple is), so it seeds none.
    let mut unique: Vec<String> = if m.is_composite_key() {
        Vec::new()
    } else {
        m.pk_field_names().iter().map(ToString::to_string).collect()
    };
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
