use super::*;
use std::collections::HashSet;

/// Check each field's `(column …)` form against what it aliases: a composite-key
/// relation names every target key part exactly once (`(column part = "col", …)`), any
/// other field takes at most the single-name form, and no aliased column is occupied by
/// another field or another part.
pub(crate) fn validate_column_aliases(
    ast: &Model,
    mi: usize,
    models: &[RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    let model = &models[mi];
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(rmem) = model.member(&f.name.node) else {
            continue;
        };
        let target = match &rmem.kind {
            MemberKind::Forward {
                target,
                custom_on: None,
                ..
            } => match index.get(target) {
                Some(&ti) => Some(&models[ti]),
                // An unknown target is already reported.
                None => continue,
            },
            _ => None,
        };
        let map = column_map(f);
        match target.filter(|t| t.is_composite_key()) {
            Some(t) if map.is_empty() => {
                if column_override(f).is_some() {
                    sink.error_note(
                        code::COLUMN_ALIAS,
                        f.span,
                        format!(
                            "`{}` references composite-key `{}` — one column name can't hold its key",
                            f.name.node, t.name
                        ),
                        format!(
                            "name the local column for each key part: `(column {})`",
                            example_map(t)
                        ),
                    );
                }
            }
            Some(t) => {
                if key_parts_complete(f, map, t, sink) {
                    check_collisions(f, map, rmem, model, models, index, sink);
                }
            }
            None if !map.is_empty() => sink.error_note(
                code::COLUMN_ALIAS,
                map[0].span,
                format!(
                    "`{}` takes no per-part column map — only a relation into a composite-key model has key parts to alias",
                    f.name.node
                ),
                "use the single-name form: `(column \"legacy_name\")`",
            ),
            None => {}
        }
    }
}

/// Every map entry names a distinct target key part, and every key part is named.
fn key_parts_complete(f: &Field, map: &[ColumnAlias], target: &RModel, sink: &mut Sink) -> bool {
    let parts = target.pk_field_names();
    let mut seen = HashSet::new();
    let mut ok = true;
    for a in map {
        if !parts.contains(&a.part.node.as_str()) {
            sink.error(
                code::COLUMN_ALIAS,
                a.part.span,
                format!(
                    "`{}` is not a primary-key part of `{}` (key: {})",
                    a.part.node,
                    target.name,
                    parts.join(", ")
                ),
            );
            ok = false;
        } else if !seen.insert(a.part.node.as_str()) {
            sink.error(
                code::COLUMN_ALIAS,
                a.part.span,
                format!("key part `{}` is aliased twice", a.part.node),
            );
            ok = false;
        }
    }
    let missing: Vec<&str> = parts.into_iter().filter(|p| !seen.contains(p)).collect();
    if ok && !missing.is_empty() {
        sink.error(
            code::COLUMN_ALIAS,
            f.span,
            format!(
                "`{}` leaves key part(s) {} of `{}` without a column",
                f.name.node,
                missing.join(", "),
                target.name
            ),
        );
        ok = false;
    }
    ok
}

/// No aliased column is named twice or already occupied by another field's column(s).
fn check_collisions(
    f: &Field,
    map: &[ColumnAlias],
    rmem: &RMember,
    model: &RModel,
    models: &[RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    let mut used = HashSet::new();
    for a in map {
        if !used.insert(a.column.as_str()) {
            sink.error(
                code::COLUMN_COLLISION,
                a.span,
                format!(
                    "column `{}` holds two key parts of `{}`",
                    a.column, f.name.node
                ),
            );
            continue;
        }
        let owner = model.members.iter().find(|other| {
            other.name != rmem.name && occupied(other, models, index).contains(&a.column)
        });
        if let Some(owner) = owner {
            sink.error(
                code::COLUMN_COLLISION,
                a.span,
                format!(
                    "column `{}` is already occupied by field `{}`",
                    a.column, owner.name
                ),
            );
        }
    }
}

/// The physical column(s) a member occupies on its own table.
fn occupied(mem: &RMember, models: &[RModel], index: &HashMap<String, usize>) -> Vec<String> {
    match &mem.kind {
        MemberKind::Scalar { column, .. } => vec![column.clone()],
        MemberKind::Forward { target, .. } => index
            .get(target)
            .map(|&ti| mem.fk_columns_into(&models[ti]))
            .unwrap_or_default()
            .into_iter()
            .map(|(column, _)| column)
            .collect(),
        MemberKind::Inverse { .. } => Vec::new(),
    }
}

/// `part = "…", …` over the target's key parts, for a fix-it note.
fn example_map(target: &RModel) -> String {
    target
        .pk_field_names()
        .iter()
        .map(|p| format!("{p} = \"…\""))
        .collect::<Vec<_>>()
        .join(", ")
}
