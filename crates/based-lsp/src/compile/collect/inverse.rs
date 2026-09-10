use super::*;


/// Every explicit inverse pairing `(Model.field)` as its `(model, field)` idents — the
/// `field` part references a forward edge on `model`. Inferred inverses carry no such
/// written pairing (they surface via `Fact.nav` instead).
pub(crate) fn collect_explicit_inverse_fields(decls: &[Decl]) -> Vec<(&Ident, &Ident)> {
    let mut out = Vec::new();
    for d in decls {
        if let Decl::Model(m) = d {
            for mem in &m.members {
                if let Member::Field(f) = mem {
                    if let Some(inv) = &f.inverse {
                        out.push((&inv.model, &inv.field));
                    }
                }
            }
        }
    }
    out
}


/// Write-target models, recursing through `tx` blocks; `raw` carries no target.
pub(crate) fn collect_write_targets<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a Ident>) {
    for w in body {
        match w {
            WriteStmt::Create { model, .. }
            | WriteStmt::Update { model, .. }
            | WriteStmt::Delete { model, .. }
            | WriteStmt::Restore { model, .. }
            | WriteStmt::HardDelete { model, .. } => out.push(model),
            WriteStmt::Tx(inner) => collect_write_targets(inner, out),
            WriteStmt::Raw(_) => {}
        }
    }
}

// ---- Field-reference path collectors (root model + its column paths) --------
// Each pushes `(root_model, segments)` so a cursor on any segment resolves through
// the relation walk in `Snapshot::walk_path`.
