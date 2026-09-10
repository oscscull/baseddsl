//! `create … from $shape`: shape-as-input eligibility at the use site.

use super::*;

mod bulk_upsert;
mod incoming;
mod input_body;
mod input_coverage;
mod input_nest;
mod input_scalar;
mod input_shape;

use bulk_upsert::*;
pub(in crate::check::mutation) use incoming::rhs_incoming_span;
use incoming::*;
use input_body::*;
use input_coverage::*;
use input_nest::*;
use input_scalar::*;
use input_shape::*;

/// Shape-as-input eligibility for every `create … from $param` in the body. A
/// `shape` is a neutral bag of typed fields; whether it may serve as a create's row input
/// is a property of the **use site** (this shape meeting this `create Model`), not of the
/// shape declaration — so the check lives here, with the mutation's param types in scope.
pub(super) fn check_from_creates(m: &Mutation, cx: &Cx, sink: &mut Sink) {
    for stmt in flatten_writes(&m.body) {
        let WriteStmt::Create {
            model,
            from: Some(cf),
            conflict,
            ..
        } = stmt
        else {
            continue;
        };
        let Some(mi) = cx.find(&model.node) else {
            continue; // unknown model reported by the write check
        };
        // A structured `create … from`'s read-back: a bulk `Model[] from` reads back
        // `-> Shape[]` (array) or `-> ok`; a single `Model from` reads back `-> Shape` (one
        // row) or `-> ok`. A `[]` mismatch between the create and the return is an error.
        if !m.ret.ack && m.ret.many != cf.bulk {
            let (want, verb) = if cf.bulk {
                (
                    "`-> Shape[]`",
                    "a bulk `create Model[] from` reads back an array",
                )
            } else {
                (
                    "`-> Shape`",
                    "a single `create Model from` reads back one row",
                )
            };
            sink.error_note(
                code::INPUT_BULK_READBACK,
                cf.span,
                format!(
                    "`create {}{} from` must return {want} or `-> ok`",
                    model.node,
                    if cf.bulk { "[]" } else { "" }
                ),
                verb,
            );
        }
        // Bulk upsert: `on conflict (target) update { … }` on a `create … from`. Reuses
        // the singular upsert's conflict-target rules, over the input shape's columns.
        if let Some(oc) = conflict {
            check_bulk_upsert(oc, mi, m, cf, cx, sink);
        }
        if let Some(shape) = resolve_input_shape(m, cf, model, cx, sink) {
            check_input_shape(
                &shape,
                mi,
                cf,
                m.scoped.as_ref(),
                m.unscoped.is_some(),
                conflict.is_some(),
                cx,
                sink,
            );
        }
    }
}

fn flatten_writes(body: &[WriteStmt]) -> Vec<&WriteStmt> {
    let mut out = Vec::new();
    for w in body {
        match w {
            WriteStmt::Tx(inner) => out.extend(inner.iter()),
            other => out.push(other),
        }
    }
    out
}
