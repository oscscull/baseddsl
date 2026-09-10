use super::*;

/// Read-only resolution context shared by every checker pass.
pub struct Cx<'a> {
    pub models: &'a [RModel],
    /// model name -> index into `models`.
    pub index: &'a HashMap<String, usize>,
    /// named filter -> its declaration (arity + body). The body is re-resolved
    /// against each call-site model, since a filter has no model of its own.
    pub filters: &'a HashMap<String, &'a NamedFilter>,
    /// shape name -> the model it projects (`from`). Used to resolve return types.
    pub shapes: &'a HashMap<String, String>,
    /// shape name -> its projection body. Lets `$ctx` collection  walk a return
    /// shape's relation reaches to find joined *scoped* models, whose `@scope` codegen
    /// injects into the join `ON` — so the callable must require their `$ctx` fields.
    pub shape_bodies: &'a HashMap<String, &'a [ShapeField]>,
    /// Resolved `scope` decls — for validating a callable's
    /// `scoped Name` acknowledgement.
    pub scopes: &'a [RScope],
    /// scope name -> index into `scopes`.
    pub scope_index: &'a HashMap<String, usize>,
    /// Resolved `enum` decls — for validating variant membership in a value position.
    pub enums: &'a [REnum],
    /// enum name -> index into `enums`.
    pub enum_index: &'a HashMap<String, usize>,
}
