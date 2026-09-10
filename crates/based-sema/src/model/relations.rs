use super::*;

/// Validate `@fk`/`@no_fk` on relation fields — the target-independent, convention-free
/// half of the FK checks (the reason-vs-convention divergence rule runs later, in the
/// manifest-dependent pass). Both decorators are valid only on a *forward to-one* relation
/// that owns a conventional `<field>_id` FK column:
///   * on an inverse / `[]` edge, or a scalar column,
///   * on a custom-join (`on:`) relation (no conventional FK column),
///   * both `@fk` and `@no_fk` on one edge,
///   * `@fk(on_delete: set_null)` on a required (non-nullable) relation,
///   * an unknown referential action
pub(crate) fn validate_fk(ast: &Model, mi: usize, models: &[RModel], sink: &mut Sink) {
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        if f.fk.is_none() && f.no_fk.is_none() {
            continue;
        }
        let span =
            f.fk.as_ref()
                .map(|a| a.span)
                .or_else(|| f.no_fk.as_ref().map(|a| a.span))
                .unwrap_or(f.span);
        // `@fk` + `@no_fk` on the same edge is contradictory.
        if f.fk.is_some() && f.no_fk.is_some() {
            sink.error(
                code::FK_CONFLICT,
                span,
                format!(
                    "`{}` carries both `@fk` and `@no_fk` — an FK is either opted in or out, not both",
                    f.name.node
                ),
            );
            continue;
        }
        // The decorators mean something only on a forward to-one relation column.
        if let Some(MemberKind::Forward { custom_on, .. }) =
            models[mi].member(&f.name.node).map(|m| &m.kind)
        {
            if custom_on.is_some() {
                sink.error(
                    code::FK_CUSTOM_JOIN,
                    span,
                    format!(
                        "`@fk`/`@no_fk` on `{}` — a custom-join (`on:`) relation owns no conventional FK column",
                        f.name.node
                    ),
                );
                continue;
            }
        } else {
            sink.error(
                code::FK_TARGET,
                span,
                format!(
                    "`@fk`/`@no_fk` on `{}` — only a forward to-one relation (which owns the `{}_id` FK column) can carry one",
                    f.name.node, f.name.node
                ),
            );
            continue;
        }
        // Referential actions: known spelling, and `set_null` needs a nullable relation.
        if let Some(fk) = &f.fk {
            for act in [&fk.on_delete, &fk.on_update].into_iter().flatten() {
                match FkAction::parse(&act.node) {
                    None => sink.error(
                        code::FK_ACTION,
                        act.span,
                        format!(
                            "unknown referential action `{}` — use cascade, restrict, set_null, or no_action",
                            act.node
                        ),
                    ),
                    Some(FkAction::SetNull) if !f.ty.optional => sink.error(
                        code::FK_SET_NULL_REQUIRED,
                        act.span,
                        format!(
                            "`set_null` on required relation `{}` — make it optional (`{}: {}?`) for the FK to null it",
                            f.name.node,
                            f.name.node,
                            type_base_name(f)
                        ),
                    ),
                    _ => {}
                }
            }
        }
    }
}

/// The relation's target model name, for a diagnostic hint (`field: Target?`).
pub(crate) fn type_base_name(f: &Field) -> String {
    match &f.ty.base {
        BaseType::Model(m) => m.node.clone(),
        _ => "Target".to_string(),
    }
}

/// Validate `@was` rename directives. A `@was` names a *previous*
/// name — one that lives only in the migration snapshot, so sema can't confirm it existed
/// (the diff does). It can catch the two locally-decidable mistakes: a no-op self-rename
/// and an old name that is still a *live* column/table ( — then it can't
/// be the rename's source). Field-level `@was` sits in the field modifier position; the
/// model-level form is a generic decorator.
pub(crate) fn validate_was(ast: &Model, mi: usize, models: &[RModel], sink: &mut Sink) {
    // Field-level: `<field>: <ty> @was("old_col")`.
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(was) = &f.was else { continue };
        let old = &was.node;
        let current = models[mi]
            .member(&f.name.node)
            .map(|m| m.physical_col().to_string());
        if current.as_deref() == Some(old.as_str()) {
            sink.error(
                code::WAS_NOOP,
                was.span,
                format!("`@was(\"{old}\")` renames `{old}` to itself — remove it"),
            );
        } else if models[mi].column(old).is_some() {
            sink.error_note(
                code::WAS_LIVE,
                was.span,
                format!(
                    "`@was(\"{old}\")` names a column that still exists in `{}`",
                    ast.name.node
                ),
                "`@was` names a *previous* column name; a live column can't be a rename source",
            );
        }
    }
    // Model-level: `@was("old_table")` decorator.
    for d in &ast.decorators {
        if d.name.node != "was" {
            continue;
        }
        let Some(DecoArg::Lit(Literal::Str(old))) = d.args.first() else {
            continue;
        };
        if *old == models[mi].table {
            sink.error(
                code::WAS_NOOP,
                d.span,
                format!("`@was(\"{old}\")` renames table `{old}` to itself — remove it"),
            );
        } else if models.iter().any(|m| &m.table == old) {
            sink.error_note(
                code::WAS_LIVE,
                d.span,
                format!("`@was(\"{old}\")` names a table that still exists"),
                "`@was` names a *previous* table name; a live table can't be a rename source",
            );
        }
    }
}

pub(crate) fn validate_relations(
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
                MemberKind::Forward { target, .. } => match index.get(target) {
                    None => sink.error(
                        code::UNKNOWN_MODEL,
                        mem.span,
                        format!("relation `{}` names unknown model `{target}`", mem.name),
                    ),
                    // A to-one relation's FK references the target's `id`; a keyless
                    // target has none, so the edge could never resolve.
                    Some(&ti) if models[ti].no_id => sink.error_note(
                        code::REL_TO_KEYLESS,
                        mem.span,
                        format!(
                            "relation `{}` targets the keyless model `{target}`",
                            mem.name
                        ),
                        "a `@no_id` model has no `id` for a foreign key to reference",
                    ),
                    Some(_) => {}
                },
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
pub(crate) fn infer_inverse(target: &RModel, me: &str) -> Result<String, String> {
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

pub(crate) fn check_inverse_ref(target: &RModel, via: &str, me: &str, mem: &RMember, sink: &mut Sink) {
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
