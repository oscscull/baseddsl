use super::*;

/// An `incoming.<relation>` operand moves a foreign key as a whole: it must be the entire
/// RHS, assigned to a relation into the same model, so every key column lines up. A
/// composite-key relation can only be set from incoming that way.
pub(super) fn check_incoming_relation(a: &Assign, model: &RModel, cx: &Cx, sink: &mut Sink) {
    let lhs_target = model.member(&a.col.node).and_then(forward_target);
    match &a.value {
        AssignRhs::Value(Value::Path(p)) if is_incoming_path(p) => {
            let field = &p.segments[1];
            let rhs_target = model.member(&field.node).and_then(forward_target);
            match (rhs_target, lhs_target) {
                (Some(rt), Some(lt)) if rt != lt => mismatch(a, field, Some(rt), sink),
                (Some(rt), None) => mismatch(a, field, Some(rt), sink),
                (None, Some(lt)) if composite_key(lt, cx) => mismatch(a, field, None, sink),
                _ => {}
            }
        }
        AssignRhs::Value(_) => {}
        AssignRhs::Arith { .. } => reject_in_arith(&a.value, model, sink),
    }
}

/// Report an `incoming.<relation>` operand nested in arithmetic.
fn reject_in_arith(rhs: &AssignRhs, model: &RModel, sink: &mut Sink) {
    match rhs {
        AssignRhs::Value(Value::Path(p)) if is_incoming_path(p) => {
            let field = &p.segments[1];
            if model.member(&field.node).and_then(forward_target).is_some() {
                sink.error_note(
                    code::INPUT_INCOMING_FIELD,
                    field.span,
                    format!("`incoming.{}` is a relation, not a number", field.node),
                    "assign a relation from incoming whole: `<relation> = incoming.<relation>`",
                );
            }
        }
        AssignRhs::Value(_) => {}
        AssignRhs::Arith { lhs, rhs, .. } => {
            reject_in_arith(lhs, model, sink);
            reject_in_arith(rhs, model, sink);
        }
    }
}

fn mismatch(a: &Assign, field: &Ident, rhs_target: Option<&str>, sink: &mut Sink) {
    let what = rhs_target.map_or_else(
        || "is not a relation".to_string(),
        |t| format!("is a relation to `{t}`"),
    );
    sink.error_note(
        code::INPUT_INCOMING_FIELD,
        field.span,
        format!(
            "`incoming.{}` {what} and can't be assigned to `{}`",
            field.node, a.col.node
        ),
        "a relation is set from incoming as a whole: `<relation> = incoming.<relation>`, both into the same model",
    );
}

/// The target model of a forward relation backed by FK column(s).
fn forward_target(mem: &RMember) -> Option<&str> {
    match &mem.kind {
        MemberKind::Forward {
            target,
            custom_on: None,
            ..
        } => Some(target.as_str()),
        _ => None,
    }
}

fn composite_key(target: &str, cx: &Cx) -> bool {
    cx.find(target)
        .is_some_and(|ti| cx.model(ti).pk_members().len() > 1)
}
