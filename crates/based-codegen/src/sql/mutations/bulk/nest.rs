use super::super::*;
use super::columns::{bulk_columns, bulk_push};

/// Lower one relation nest of an input shape:
/// - **FK link** (`rel { key }`) — set the FK column(s) from the payload.
/// - **to-one nested write** (a forward edge, `rel { …payload }`) — create the child first
///   (`nested_one`), the parent's FK sourced from its key ([`BulkSource::NestedOneId`]).
/// - **to-many nested write** (an inverse edge) — create the child collection after the
///   parent (`nested_many`), each child's back-FK sourced from the parent's key
///   ([`BulkSource::ParentId`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn bulk_nest_col(
    cx: &LowerCx,
    model: &RModel,
    field: &Ident,
    nest_body: &[ShapeField],
    columns: &mut Vec<BulkCol>,
    have: &mut Vec<String>,
    nested_one: &mut Vec<NestedCreate>,
    nested_many: &mut Vec<NestedCreate>,
) {
    let Some(mem) = model.member(&field.node) else {
        return;
    };
    match &mem.kind {
        MemberKind::Forward { .. } => {
            forward_nest(cx, mem, field, nest_body, columns, have, nested_one);
        }
        MemberKind::Inverse { target, via } => {
            inverse_nest(cx, field, target, via, nest_body, nested_many);
        }
        MemberKind::Scalar { .. } => {}
    }
}

/// A forward-edge nest: an FK link sets the FK columns from the payload; a to-one nested
/// write creates the child first and sources the parent's FK from its key.
fn forward_nest(
    cx: &LowerCx,
    mem: &RMember,
    field: &Ident,
    nest_body: &[ShapeField],
    columns: &mut Vec<BulkCol>,
    have: &mut Vec<String>,
    nested_one: &mut Vec<NestedCreate>,
) {
    let Some(target) = mem.kind.target() else {
        return;
    };
    let Some(child) = cx.schema.model(target) else {
        return;
    };
    let key_fields: Vec<String> = child
        .pk_field_names()
        .iter()
        .map(ToString::to_string)
        .collect();
    // An FK link: the block names only the target's key part(s) → FK from payload.
    if based_sema::is_key_link(nest_body, &key_fields) {
        for (fk_col, part) in cx.schema.fk_columns(mem) {
            let src = BulkSource::FkPart {
                relation: field.node.clone(),
                key_field: part.name.clone(),
            };
            bulk_push(columns, have, fk_col, src);
        }
        return;
    }
    // A to-one nested write: create the child, link the parent's FK to its key.
    let child_bulk = build_bulk_insert(cx, child, nest_body, String::new(), false);
    for (fk_col, part) in cx.schema.fk_columns(mem) {
        let src = BulkSource::NestedOneId {
            nest: field.node.clone(),
            key_field: part.name.clone(),
        };
        bulk_push(columns, have, fk_col, src);
    }
    nested_one.push(NestedCreate {
        relation: field.node.clone(),
        child: child_bulk,
    });
}

/// A to-many inverse nest: create the child rows after the parent, each linked back through
/// the child's forward edge (`via`) to the parent's key.
fn inverse_nest(
    cx: &LowerCx,
    field: &Ident,
    target: &str,
    via: &str,
    nest_body: &[ShapeField],
    nested_many: &mut Vec<NestedCreate>,
) {
    let Some(child) = cx.schema.model(target) else {
        return;
    };
    let Some(via_mem) = child.member(via) else {
        return;
    };
    let mut child_bulk = build_bulk_insert(cx, child, nest_body, String::new(), false);
    // The back-FK columns aren't in the child shape — inject them, sourced from the parent's
    // key part each references.
    for (fk_col, part) in cx.schema.fk_columns(via_mem) {
        if !child_bulk.columns.iter().any(|c| c.column == fk_col) {
            child_bulk.columns.push(BulkCol {
                column: fk_col,
                source: BulkSource::ParentId {
                    key_field: part.name.clone(),
                },
            });
        }
    }
    nested_many.push(NestedCreate {
        relation: field.node.clone(),
        child: child_bulk,
    });
}

/// The primary-key parts of a model and how to recover each per row after its insert (so a
/// parent can link its FK to a nested-write child's key).
fn bulk_pk_parts(model: &RModel, serial_col: Option<&str>) -> Vec<PkPart> {
    model
        .pk_members()
        .iter()
        .map(|p| {
            let column = p.physical_col().to_string();
            PkPart {
                field: p.name.clone(),
                serial: serial_col == Some(column.as_str()),
                column,
            }
        })
        .collect()
}

/// Build a [`BulkInsert`] for a model from an input-shape body — the shared core of the
/// top-level `create … from` and every nested-write child. Conflict/read-back tails are set
/// by the top-level caller ([`lower_bulk_create`]); a child carries none.
pub(super) fn build_bulk_insert(
    cx: &LowerCx,
    model: &RModel,
    body: &[ShapeField],
    param: String,
    bulk: bool,
) -> BulkInsert {
    let (columns, serial_col, nested_one, nested_many) = bulk_columns(cx, model, body);
    let pk_parts = bulk_pk_parts(model, serial_col.as_deref());
    BulkInsert {
        model: model.name.clone(),
        table: cx
            .dialect
            .quote_table(model.schema.as_deref(), &model.table),
        param,
        bulk,
        columns,
        returning: serial_col.into_iter().collect(),
        conflict_tail: None,
        readback_key: Vec::new(),
        readback_serial: false,
        nested_one,
        nested_many,
        pk_parts,
    }
}
