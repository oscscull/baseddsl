//! based-sema — semantic analysis over the closed declaration set.
//!
//! Builds one resolved schema from many parsed files, then checks it:
//!   * name + casing resolution (D7): `Order` <-> model `order`, paths, inverses
//!   * implicit columns (D2): id / created_at / updated_at
//!   * type checking: fields, shape paths, predicate operands, param bindings
//!   * the four query inferences (queries.md): verb, param type, filter, target
//!   * lints: missing/useless index, nondeterministic sort, raw soft-delete gaps
//!
//! Output is a checked schema (the IR seed for codegen) plus diagnostics.

use based_ast::Decl;
use based_diagnostics::Diagnostic;

/// A checked, cross-linked schema. Fields land as resolution is implemented.
#[derive(Debug, Default)]
pub struct CheckedSchema {
    // TODO(sema-milestone): resolved models, shapes, queries; symbol tables.
}

/// Resolve and check the whole declaration set (gathered from every `.bsl` file).
pub fn check(_decls: &[Decl]) -> (CheckedSchema, Vec<Diagnostic>) {
    todo!("sema — typecheck milestone")
}
