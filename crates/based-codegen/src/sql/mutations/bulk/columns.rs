use super::super::*;
use super::nest::bulk_nest_col;

/// Resolve a `create … from $param`'s input shape to `(from_model, body)`, via the param's
/// declared shape type. Sema already validated eligibility; this just re-reads the shape.
pub(super) fn resolve_from_shape<'a>(cx: &LowerCx<'a>, param: &str) -> Option<(&'a str, &'a [ShapeField])> {
    let p = cx.params.iter().find(|p| p.name.node == param)?;
    let BaseType::Model(name) = &p.ty.as_ref()?.base else {
        return None;
    };
    cx.decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name.node => {
            Some((s.from.node.as_str(), s.body.as_slice()))
        }
        _ => None,
    })
}

/// The presence-driven INSERT column plan for a structured shape-input create, plus the
/// DB-generated `serial` id column to `RETURNING` (if any): a column the shape names is
/// written verbatim from the payload (except a `@scope` column, always the caller's `$ctx`),
/// an absent engine-managed column is filled (mint / DB-gen / `now()`), and a nested relation
/// becomes FK columns or a nested-write child.
pub(super) fn bulk_columns(
    cx: &LowerCx,
    model: &RModel,
    body: &[ShapeField],
) -> (
    Vec<BulkCol>,
    Option<String>,
    Vec<NestedCreate>,
    Vec<NestedCreate>,
) {
    let scope_terms: Vec<(String, String)> = cx
        .inject
        .iter()
        .find(|si| si.model == model.name)
        .map_or(Vec::new(), |si| si.terms.clone());
    let scope_cols: Vec<String> = scope_terms
        .iter()
        .map(|(f, _)| physical_col(model, f))
        .collect();

    // 1. Columns the input shape names — verbatim from the payload (a `@scope` column is
    //    skipped; step 2 injects it from `$ctx`).
    let mut columns: Vec<BulkCol> = Vec::new();
    let mut have: Vec<String> = Vec::new();
    let mut nested_one: Vec<NestedCreate> = Vec::new();
    let mut nested_many: Vec<NestedCreate> = Vec::new();
    for f in body {
        if let Some((col, src)) = named_bulk_col(model, f) {
            if !scope_cols.contains(&col) {
                bulk_push(&mut columns, &mut have, col, src);
            }
        } else if let Some((field, nest_body)) = nest_field(cx, f) {
            bulk_nest_col(
                cx,
                model,
                field,
                nest_body,
                &mut columns,
                &mut have,
                &mut nested_one,
                &mut nested_many,
            );
        }
    }

    // 2. `@scope` — always the caller's `$ctx.<field>`, identical for every row.
    for (field, ctx_field) in &scope_terms {
        let src = BulkSource::Ctx {
            ctx_field: ctx_field.clone(),
        };
        bulk_push(&mut columns, &mut have, physical_col(model, field), src);
    }

    // 3. The primary key when the shape does not name it: an app-minted `uuid`/`ulid` per
    //    row, or a DB-generated `serial` (omit — the DB assigns it). A `@key` / keyless
    //    model has no surrogate id (its key is an ordinary named column).
    let serial_col = serial_return_col(model, cx.dialect);
    if !model.no_id && model.key.is_empty() && !have.iter().any(|c| c == "id") {
        match model.pk_strategy() {
            Some(PkStrategy::Serial) => { /* DB-generated — omit */ }
            Some(PkStrategy::Ulid) => {
                bulk_push(
                    &mut columns,
                    &mut have,
                    "id".to_string(),
                    BulkSource::MintUlid,
                );
            }
            _ => bulk_push(
                &mut columns,
                &mut have,
                "id".to_string(),
                BulkSource::MintUuid,
            ),
        }
    }

    // 4. `@created`/`@updated` stamps the shape did not name → `CURRENT_TIMESTAMP`.
    for col in timestamp_cols(model, &[model.created.as_deref(), model.updated.as_deref()]) {
        bulk_push(&mut columns, &mut have, col, BulkSource::Now);
    }

    (columns, serial_col, nested_one, nested_many)
}

/// A relation nest in an input shape, as `(field, body)` — an inline `Nest` or a resolved
/// `NestRef`. `None` for a non-nest field.
fn nest_field<'a>(cx: &LowerCx<'a>, f: &'a ShapeField) -> Option<(&'a Ident, &'a [ShapeField])> {
    match f {
        ShapeField::Nest { field, body } => Some((field, body.as_slice())),
        ShapeField::NestRef { field, shape } => {
            shape_body_by_name(cx, &shape.node).map(|b| (field, b))
        }
        _ => None,
    }
}

/// Resolve a shape body by name from the raw decls (for a `NestRef`).
fn shape_body_by_name<'a>(cx: &LowerCx<'a>, name: &str) -> Option<&'a [ShapeField]> {
    cx.decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name => Some(s.body.as_slice()),
        _ => None,
    })
}

/// Append a bulk-insert column unless its physical column is already present (first source
/// wins — the shape-named value beats a later engine default).
pub(super) fn bulk_push(columns: &mut Vec<BulkCol>, have: &mut Vec<String>, col: String, source: BulkSource) {
    if !have.contains(&col) {
        have.push(col.clone());
        columns.push(BulkCol {
            column: col,
            source,
        });
    }
}

/// The `(physical_col, BulkSource::Field)` a scalar input-shape field writes — a bare column
/// or a single-column rename. `None` for a relation nest (handled by the caller's FK
/// expansion) or any form sema rejected.
fn named_bulk_col(model: &RModel, f: &ShapeField) -> Option<(String, BulkSource)> {
    match f {
        ShapeField::Bare(id) => Some((
            physical_col(model, &id.node),
            BulkSource::Field {
                json_key: id.node.clone(),
                field: id.node.clone(),
            },
        )),
        ShapeField::Rename {
            out,
            value: ShapeValue::Path(p),
        } if p.segments.len() == 1 => {
            let field = p.segments[0].node.clone();
            Some((
                physical_col(model, &field),
                BulkSource::Field {
                    json_key: out.node.clone(),
                    field,
                },
            ))
        }
        _ => None,
    }
}
