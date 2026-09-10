use super::*;

/// Emit E0323 for every `$…` operand in a predicate (a shape carries no parameters).
/// Returns whether any was found, so the caller can skip the ordinary param-aware check.
pub(crate) fn reject_pred_params(pred: &Predicate, sink: &mut Sink) -> bool {
    let mut found = false;
    let mut check = |v: &Value| {
        if let Value::Param(pr) = v {
            sink.error_note(
                code::CFIELD_NO_PARAMS,
                pr.name.span,
                format!(
                    "`${}` — a computed shape field has no parameters",
                    pr.name.node
                ),
                "computed-field operands are the row's own columns and literals",
            );
            found = true;
        }
    };
    walk_pred_values(pred, &mut check);
    found
}

/// Visit every `Value` leaf of a predicate.
pub(crate) fn walk_pred_values(pred: &Predicate, f: &mut impl FnMut(&Value)) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            walk_pred_values(a, f);
            walk_pred_values(b, f);
        }
        Predicate::Not(inner) => walk_pred_values(inner, f),
        Predicate::Cmp { value, .. } => f(value),
        Predicate::InList { values, .. } => values.iter().for_each(f),
        Predicate::FilterCall { args, .. } => args.iter().for_each(f),
        Predicate::Bare(_) | Predicate::Raw(_) => {}
    }
}

/// Type-check one `Cmp`: the operator must apply to the left operand's type, and
/// (for equality/ordering against a literal or another column) the two operands
/// must share a family. Silent when either side failed to resolve — that name
/// error was already reported.
pub(crate) fn check_cmp_types(path: &Path, op: Op, value: &Value, mi: usize, cx: &Cx, sink: &mut Sink) {
    let Some(lhs) = resolve_quiet(path, mi, cx) else {
        return;
    };
    let Some(seg) = path.segments.last() else {
        return;
    };
    let span = seg.span;
    if reject_opaque(&lhs, path, "filter", sink) {
        return;
    }

    // 1. Operator applicability on the left operand.
    match op {
        Op::Like => {
            if !matches!(lhs, Terminal::Scalar(Primitive::Text)) {
                sink.error(
                    code::OP_TYPE,
                    span,
                    format!(
                        "`~` (like) needs a text column, but {} is not text",
                        terminal_name(&lhs)
                    ),
                );
                return;
            }
        }
        Op::Gt | Op::Lt | Op::Ge | Op::Le => {
            if matches!(
                terminal_family(&lhs),
                Family::Bool | Family::Json | Family::Key | Family::Binary
            ) {
                sink.error(
                    code::OP_TYPE,
                    span,
                    format!(
                        "`{}` needs an orderable column, but {} is not",
                        op_sym(op),
                        terminal_name(&lhs)
                    ),
                );
                return;
            }
        }
        // `in`/`has` are collection/json containment: the right operand's element
        // type differs from the column, so family compatibility does not apply.
        Op::In | Op::Has => return,
        Op::Eq | Op::Ne => {}
    }

    // 2. Operand family compatibility against a literal or another column.
    let lf = terminal_family(&lhs);
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved RHS: name error already reported
        },
        // A param's type is checked at its declaration (check_param); a `$ctx.field`
        // is typed by inference from *this* comparison (ctx.rs), so it never clashes
        // here; a function's return type is not modelled yet.
        Value::Param(_) | Value::Func(_) => return,
    };
    if !compatible(lf, rf) {
        sink.error(
            code::CMP_TYPE,
            span,
            format!(
                "cannot compare {} to a {} value",
                terminal_name(&lhs),
                family_name(rf)
            ),
        );
    }
}

/// Type-check one `in` value-list element: unlike the single-bind `in $param`
/// form (whose RHS the engine can't see into), a listed element is compared to
/// the column with `=` semantics, so it must share the column's family — the
/// per-element twin of `check_cmp_types` step 2.
pub(crate) fn check_in_element_type(path: &Path, value: &Value, mi: usize, cx: &Cx, sink: &mut Sink) {
    let Some(lhs) = resolve_quiet(path, mi, cx) else {
        return;
    };
    let Some(seg) = path.segments.last() else {
        return;
    };
    if reject_opaque(&lhs, path, "filter", sink) {
        return;
    }
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved element: name error already reported
        },
        // Same skips as the single-value comparison: params are typed at their
        // declaration, `$ctx` by inference, functions are unmodelled.
        Value::Param(_) | Value::Func(_) => return,
    };
    if !compatible(terminal_family(&lhs), rf) {
        sink.error(
            code::CMP_TYPE,
            seg.span,
            format!(
                "cannot compare {} to a {} value",
                terminal_name(&lhs),
                family_name(rf)
            ),
        );
    }
}

pub(crate) fn op_sym(op: Op) -> &'static str {
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

/// Check one predicate against an optional model context. `params` is the set of
/// legal `$`-parameter names (besides `$ctx`, always allowed). When `model`
/// is `None` (a named filter checked at its declaration, without a caller), column
/// paths are not bound to a model — only params, filter calls, and functions are
/// checked; the body's columns resolve later at each call site (see below).
pub fn check_predicate(
    pred: &Predicate,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    check_predicate_in(pred, model, cx, params, &mut Vec::new(), sink);
}

/// `col <op> value`. When the left column is enum-typed, the right operand is a variant
/// (or a param), not a column — membership-checked instead of resolved as a path, which
/// would misread the variant as an unknown field.
pub(crate) fn check_cmp(
    path: &Path,
    op: Op,
    value: &Value,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let mut handled = false;
    if let Some(mi) = model {
        resolve_path(path, mi, cx, sink);
        if let Some(en) = cx.terminal_enum(path, mi) {
            // Ordered comparison is numeric-only: allowed on an int enum, rejected on a
            // string enum (its values have no order).
            if matches!(op, Op::Gt | Op::Lt | Op::Ge | Op::Le) && !en.is_int() {
                sink.error(
                    code::ENUM_ORDERED_OP,
                    path.segments.last().map_or(en.span, |s| s.span),
                    format!(
                        "`{}` is a string enum; ordered comparison is only valid on a \
                         numeric enum",
                        en.name
                    ),
                );
            }
            handled = check_enum_operand(value, en, params, sink);
        }
    }
    if !handled {
        check_value(value, model, cx, params, sink);
        // Operand typing runs after both sides' name errors are reported, and is silent
        // when either side failed to resolve.
        if let Some(mi) = model {
            check_cmp_types(path, op, value, mi, cx, sink);
        }
    }
}

/// `col in (…)`. Against an enum column each bare element is a variant — membership-
/// checked instead of resolved as a column path.
pub(crate) fn check_in_list(
    path: &Path,
    values: &[Value],
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let en = model.and_then(|mi| {
        resolve_path(path, mi, cx, sink);
        cx.terminal_enum(path, mi)
    });
    for value in values {
        if en.is_some_and(|en| check_enum_operand(value, en, params, sink)) {
            continue;
        }
        check_value(value, model, cx, params, sink);
        if let Some(mi) = model {
            check_in_element_type(path, value, mi, cx, sink);
        }
    }
}

/// `filter(args…)`. On matching arity the filter's body is resolved against the call-site
/// model, so its column paths (`address.city = …`) are checked against the model the query
/// actually runs on — a filter has no model of its own. The arguments themselves are
/// values in the *caller's* param scope.
pub(crate) fn check_filter_call(
    name: &Ident,
    args: &[Value],
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    match cx.filters.get(&name.node) {
        None => sink.error(
            code::UNKNOWN_FILTER,
            name.span,
            format!("unknown filter `{}`", name.node),
        ),
        Some(def) if def.params.len() != args.len() => sink.error(
            code::FILTER_ARITY,
            name.span,
            format!(
                "filter `{}` takes {} argument(s), got {}",
                name.node,
                def.params.len(),
                args.len()
            ),
        ),
        Some(def) => resolve_filter_body(def, model, cx, in_filters, sink),
    }
    for v in args {
        check_value(v, model, cx, params, sink);
    }
}

/// Inner walker carrying `in_filters`, the stack of named filters currently being
/// expanded, so a filter that (directly or transitively) calls itself terminates
/// instead of recursing forever.
pub(crate) fn check_predicate_in(
    pred: &Predicate,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    match pred {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            check_predicate_in(a, model, cx, params, in_filters, sink);
            check_predicate_in(b, model, cx, params, in_filters, sink);
        }
        Predicate::Not(p) => check_predicate_in(p, model, cx, params, in_filters, sink),
        Predicate::Cmp { path, op, value } => {
            check_cmp(path, *op, value, model, cx, params, sink);
        }
        Predicate::InList { path, values } => {
            check_in_list(path, values, model, cx, params, sink);
        }
        Predicate::Bare(path) => {
            // A bare atom is a bool column or a zero-arg named-filter reference.
            if path.segments.len() == 1 {
                if let Some(def) = cx.filters.get(&path.segments[0].node) {
                    resolve_filter_body(def, model, cx, in_filters, sink);
                    return;
                }
            }
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
        }
        Predicate::FilterCall { name, args } => {
            check_filter_call(name, args, model, cx, params, in_filters, sink);
        }
        Predicate::Raw(raw) => {
            check_raw_params(raw, params, sink);
            if let Some(mi) = model {
                if let Some(sd) = &cx.model(mi).soft_delete {
                    sink.warn(
                        code::RAW_SOFT_DELETE_GAP,
                        raw.span,
                        format!(
                            "raw SQL on soft-delete model `{}`: engine can't verify the `{}` tombstone filter — confirm it",
                            cx.model(mi).name, sd.field
                        ),
                    );
                }
            }
        }
    }
}

/// Re-resolve a named filter's body against the call-site `model`. The filter's
/// *own* params are the legal `$`-set inside its body (a filter param is referenced
/// as `$c`, same `$`-means-bound rule as everywhere else).
/// `in_filters` guards against a filter that expands to itself. With no call-site
/// model (`model` is `None`, e.g. a filter reached while checking another filter's
/// declaration) there is nothing to resolve columns against, so this is a no-op.
pub(crate) fn resolve_filter_body(
    def: &NamedFilter,
    model: Option<usize>,
    cx: &Cx,
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    if model.is_none() || in_filters.iter().any(|n| n == &def.name.node) {
        return;
    }
    let fparams: Vec<String> = def.params.iter().map(|p| p.name.node.clone()).collect();
    in_filters.push(def.name.node.clone());
    check_predicate_in(&def.pred, model, cx, &fparams, in_filters, sink);
    in_filters.pop();
}
