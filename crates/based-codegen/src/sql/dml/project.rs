//! Assemble the SELECT projection list from a return shape: columns, FK reaches, and
//! to-one/to-many nest dispatch.

use super::*;

/// Build the indented SELECT-list text for a query. Delegates to [`project_return`],
/// the shape-projection core the write side reuses for its post-write re-select.
pub(crate) fn build_projection<'a>(
    sel: &mut Select<'a>,
    decls: &'a [Decl],
    rq: &RQuery,
    root: &'a RModel,
) -> String {
    project_return(sel, decls, rq.ret_shape.as_deref(), &rq.target, root)
}

/// Build the indented SELECT-list text from a return type. Uses the named return shape
/// when present, else projects every stored column of a bare-model return. Shared by
/// the read side (queries) and the write side (`mutations`'s declared-shape re-select),
/// so a mutation returns the *same* projection a `get` of that shape would.
pub(crate) fn project_return<'a>(
    sel: &mut Select<'a>,
    decls: &'a [Decl],
    ret_shape: Option<&str>,
    target: &str,
    root: &'a RModel,
) -> String {
    let mut cols: Vec<String> = Vec::new();
    match ret_shape {
        Some(name) => {
            if let Some(shape) = find_shape(decls, name, target) {
                let root_alias = sel.root_alias.clone();
                project_body(sel, &shape.body, root, &root_alias, "", "", &mut cols);
            }
        }
        None => {
            // Bare-model return: every stored column, aliased to its field name.
            for mem in &root.members {
                match &mem.kind {
                    MemberKind::Scalar { column, .. } => {
                        cols.push(format!(
                            "{} AS {}",
                            sel.qcol(&sel.root_alias, column),
                            sel.q(&mem.name)
                        ));
                    }
                    MemberKind::Forward { .. } => {
                        // A single-column FK projects its flat value; a composite FK projects
                        // one column per key part under a `<field>.<part>` alias, so the
                        // runtime reassembles the structured id object.
                        let root_alias = sel.root_alias.clone();
                        push_fk_projection(sel, mem, &root_alias, &mem.name, &mut cols);
                    }
                    MemberKind::Inverse { .. } => {}
                }
            }
        }
    }
    cols.iter()
        .map(|c| format!("  {c}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

/// Lower one relation nest (`field { body }`, or a `field -> Shape` expansion): a
/// to-one edge projects the child's columns under a `field.`-prefixed alias; a
/// to-many edge aggregates the child rows into a JSON-array column (`field[]`).
#[allow(clippy::too_many_arguments)]
fn project_nest<'a>(
    sel: &mut Select<'a>,
    field: &Ident,
    body: &'a [ShapeField],
    model: &'a RModel,
    alias: &str,
    prefix: &str,
    out_prefix: &str,
    cols: &mut Vec<String>,
) {
    if let Some((child_alias, child_prefix, child_model)) =
        sel.enter_to_one(&field.node, alias, prefix, model)
    {
        let nested_out = format!("{out_prefix}{}{NEST_SEP}", field.node);
        if to_one_absent_possible(model, &field.node) {
            cols.push(format!(
                "{} AS {}",
                sel.qcol(&child_alias, &pk_col(child_model)),
                sel.q(&format!("{nested_out}{NEST_PRESENT}"))
            ));
        }
        project_body(
            sel,
            body,
            child_model,
            &child_alias,
            &child_prefix,
            &nested_out,
            cols,
        );
    } else if let Some((child_model, via_field, edge_sort)) = sel.to_many_edge(&field.node, model) {
        let arr = sel.json_array_subquery(body, child_model, &via_field, alias, model, edge_sort);
        let out = out_alias(out_prefix, &format!("{}{ARRAY_MARK}", field.node));
        cols.push(format!("{arr} AS {}", sel.q(&out)));
    }
}

/// Prepend the nest output prefix to a field's output alias (`""` at the top,
/// `"buyer."` inside a nest → `"buyer.name"`).
fn out_alias(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
}

/// Project a forward-relation FK under `out_field`. A single-column FK is a flat scalar
/// column; a composite FK (a relation into a `@key(f1, f2, …)` model) projects one column
/// per key part under a `<out_field>.<part>` alias, so the runtime reassembles the
/// structured id object (`{ order: …, product: … }`).
fn push_fk_projection(
    sel: &Select,
    mem: &RMember,
    alias: &str,
    out_field: &str,
    cols: &mut Vec<String>,
) {
    let pairs: Vec<(String, String)> = sel
        .schema
        .fk_columns(mem)
        .into_iter()
        .map(|(c, part)| (c, part.name.clone()))
        .collect();
    if pairs.len() <= 1 {
        let fk = pairs
            .first()
            .map_or_else(|| mem.physical_col().to_string(), |(c, _)| c.clone());
        cols.push(format!("{} AS {}", sel.qcol(alias, &fk), sel.q(out_field)));
    } else {
        for (fk, part) in &pairs {
            cols.push(format!(
                "{} AS {}",
                sel.qcol(alias, fk),
                sel.q(&format!("{out_field}{NEST_SEP}{part}"))
            ));
        }
    }
}

/// Project a shape body against `model`, appending `expr AS out` lines to `cols`.
///
/// `alias`/`prefix` locate the model in the join graph (the root alias + empty prefix
/// at the top; a joined alias + its path prefix inside a nest), so paths resolve from
/// the right table. `out_prefix` is prepended to every emitted output alias — empty at
/// the top, `field.` (one [`NEST_SEP`] per level) inside a nest — so a nested column
/// lands under a `parent.child` alias the runtime reassembles into a sub-object.
fn project_body<'a>(
    sel: &mut Select<'a>,
    fields: &'a [ShapeField],
    model: &'a RModel,
    alias: &str,
    prefix: &str,
    out_prefix: &str,
    cols: &mut Vec<String>,
) {
    for f in fields {
        match f {
            ShapeField::Bare(id) => {
                // A bare composite-FK field projects as a structured id object (one column
                // per key part); every other bare field is a single scalar column.
                if let Some(mem) = model.member(&id.node) {
                    if matches!(mem.kind, MemberKind::Forward { .. })
                        && sel.schema.fk_columns(mem).len() > 1
                    {
                        push_fk_projection(sel, mem, alias, &out_alias(out_prefix, &id.node), cols);
                        continue;
                    }
                }
                let (a, col) = sel.resolve_from(&single(&id.node), alias, prefix, model);
                cols.push(format!(
                    "{} AS {}",
                    sel.qcol(&a, &col),
                    sel.q(&out_alias(out_prefix, &id.node))
                ));
            }
            ShapeField::Rename { out, value } => {
                project_rename(sel, out, value, model, alias, prefix, out_prefix, cols);
            }
            // A to-**one** relation nests the target's columns under a `field.`-prefixed
            // alias (reassembled by the runtime). A to-**many** relation aggregates the
            // child rows into a single JSON-array column (`field[]`) via a correlated
            // subquery, parsed back into an array by the runtime.
            ShapeField::Nest { field, body } => {
                project_nest(sel, field, body, model, alias, prefix, out_prefix, cols);
            }
            // `field -> Shape`: same lowering as an inline nest, the body coming from
            // the named shape's decl. Sema rejects reference cycles; the stack guard
            // keeps this terminating on an unchecked schema.
            ShapeField::NestRef { field, shape } => {
                let Some(body) = sel.enter_shape_ref(&shape.node) else {
                    continue;
                };
                project_nest(sel, field, body, model, alias, prefix, out_prefix, cols);
                sel.exit_shape_ref();
            }
            // `out = edge.far { body }`: flatten a to-many path to the distinct far
            // side, hiding the junction — a JSON-array column (`out[]`) the runtime
            // parses like any to-many nest.
            ShapeField::Flatten { out, path, body } => {
                if let Some(arr) = sel.json_flatten_subquery(body, path, alias, model) {
                    let name = out_alias(out_prefix, &format!("{}{ARRAY_MARK}", out.node));
                    cols.push(format!("{arr} AS {}", sel.q(&name)));
                }
            }
            ShapeField::Spread { .. } => unreachable!("spreads expanded before codegen"),
        }
    }
}

/// Project a renamed field (`out = <value>`): a path reach, a raw expression, an aggregate
/// (defensive — an aggregate shape lowers via `lower_agg_query`, never this path), or a
/// per-row computed scalar. One SELECT-list column under the `out`-named alias.
#[allow(clippy::too_many_arguments)]
fn project_rename<'a>(
    sel: &mut Select<'a>,
    out: &Ident,
    value: &'a ShapeValue,
    model: &'a RModel,
    alias: &str,
    prefix: &str,
    out_prefix: &str,
    cols: &mut Vec<String>,
) {
    match value {
        ShapeValue::Path(p) => {
            let (a, col) = sel.resolve_from(p, alias, prefix, model);
            cols.push(format!(
                "{} AS {}",
                sel.qcol(&a, &col),
                sel.q(&out_alias(out_prefix, &out.node))
            ));
        }
        ShapeValue::Raw(raw) => {
            cols.push(format!(
                "({}) AS {}",
                render_raw(sel.dialect, raw, alias, &model.table),
                sel.q(&out_alias(out_prefix, &out.node))
            ));
        }
        ShapeValue::Agg(agg) => {
            let d = sel.dialect;
            let expr = agg_sql(sel, model, alias, prefix, agg, d, true);
            cols.push(format!(
                "{expr} AS {}",
                sel.q(&out_alias(out_prefix, &out.node))
            ));
        }
        ShapeValue::Computed(expr) => {
            let sql = sel.shape_expr(expr, alias, prefix, model);
            cols.push(format!(
                "{sql} AS {}",
                sel.q(&out_alias(out_prefix, &out.node))
            ));
        }
    }
}

/// Whether a to-one nest's joined row can be absent: an optional forward relation
/// or a to-one inverse — the LEFT-JOINed edges. A required forward edge inner-joins,
/// so its row always exists. Mirrors the client emitter's `Option<…>` typing.
pub(crate) fn to_one_absent_possible(model: &RModel, field: &str) -> bool {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Forward { optional, .. }) => *optional,
        Some(MemberKind::Inverse { .. }) => true,
        _ => false,
    }
}
