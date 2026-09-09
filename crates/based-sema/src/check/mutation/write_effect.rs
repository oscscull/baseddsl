use super::*;

/// A write's effect on its target row, for the `-> ok` / declared-shape rules.
pub(super) enum WriteEffect<'a> {
    /// A real DELETE — plain-model `delete` or `hard delete`: the row is removed.
    RealDelete(&'a Ident),
    /// A whole-table wipe — `delete all` on a soft-delete model (tombstones every row).
    /// Like a real DELETE for the return rules (no single row to read back → `-> ok`,
    /// a declared shape is an error), even though the SQL is an UPDATE.
    Wipe(&'a Ident),
    /// create / update / restore / soft `delete` (tombstone): a row survives to
    /// read back.
    Surviving(&'a Ident),
    /// A raw write — its effect is outside the engine's knowledge.
    Raw,
}

/// Classify each write of the body ( `tx` blocks flattened, execution order).
pub(super) fn write_effects<'a>(body: &'a [WriteStmt], cx: &Cx) -> Vec<WriteEffect<'a>> {
    let mut out = Vec::new();
    for stmt in body {
        match stmt {
            WriteStmt::Create { model, .. } => out.push(WriteEffect::Surviving(model)),
            WriteStmt::Update { model, .. } => out.push(WriteEffect::Surviving(model)),
            WriteStmt::Restore { model, .. } => out.push(WriteEffect::Surviving(model)),
            WriteStmt::HardDelete { model, .. } => out.push(WriteEffect::RealDelete(model)),
            WriteStmt::Delete { model, where_ } => {
                let soft = cx
                    .find(&model.node)
                    .is_some_and(|mi| cx.model(mi).soft_delete.is_some());
                match (soft, where_.is_none()) {
                    // Soft `delete all` tombstones every row — an ack wipe, not a
                    // read-backable surviving write.
                    (true, true) => out.push(WriteEffect::Wipe(model)),
                    (true, false) => out.push(WriteEffect::Surviving(model)),
                    (false, _) => out.push(WriteEffect::RealDelete(model)),
                }
            }
            WriteStmt::Tx(inner) => out.extend(write_effects(inner, cx)),
            WriteStmt::Raw(_) => out.push(WriteEffect::Raw),
        }
    }
    out
}
