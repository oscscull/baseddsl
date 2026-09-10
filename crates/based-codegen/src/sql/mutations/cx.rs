use std::collections::HashMap;

use super::*;

/// The immutable lowering context for a mutation body: schema/decls/dialect, the
/// `unscoped` + chosen-scope decisions, the return model, and the precomputed per-binding
/// read-back columns. Threaded through every write of the body.
pub(crate) struct LowerCx<'a> {
    pub(crate) schema: &'a CheckedSchema,
    pub(crate) decls: &'a [Decl],
    pub(crate) dialect: Dialect,
    pub(crate) unscoped: bool,
    pub(crate) inject: &'a [ScopeInject],
    pub(crate) ret_model: &'a str,
    pub(crate) binding_refs: &'a HashMap<String, Vec<CaptureCol>>,
    /// The mutation's params — a `create … from $param`'s input shape is resolved through
    /// the param's declared type.
    pub(crate) params: &'a [Param],
}
