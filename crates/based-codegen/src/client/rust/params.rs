use super::*;

/// The input fields for a callable: one per signature param, typed from its
/// explicit annotation or inferred from the column it maps to.
pub(super) fn input_fields(schema: &CheckedSchema, c: &Callable) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = c
        .params
        .iter()
        .map(|p| (p.name.node.clone(), param_type(schema, c, p)))
        .collect();
    // Page control: a keyset page takes the opaque cursor back, an offset page an
    // explicit offset. Both optional — absence is the first page.
    match c.page {
        PageInput::Keyset => fields.push(("cursor".into(), "Option<Cursor>".into())),
        PageInput::Offset => fields.push(("offset".into(), "Option<i64>".into())),
        PageInput::None => {}
    }
    fields
}

/// How a query paginates, for its input page-control field.
pub(super) fn page_input(q: &Query) -> PageInput {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return PageInput::None,
    };
    clauses
        .iter()
        .find_map(|c| match c {
            Clause::Page(p) if p.offset => Some(PageInput::Offset),
            Clause::Page(_) => Some(PageInput::Keyset),
            _ => None,
        })
        .unwrap_or(PageInput::None)
}

/// A param's Rust type. An **entity id** — a model-typed annotation, or a param the
/// front end resolved to a relation/`id` (`param_entity`) — is the phantom-typed
/// `Id<entity::M>`; otherwise an explicit annotation wins, else infer from the
/// bound/same-named column. A param with a default (or an optional annotation)
/// becomes `Option<T>` — the client may omit it and let the engine apply the
/// default.
fn param_type(schema: &CheckedSchema, c: &Callable, p: &Param) -> String {
    // A `name?` optional filter param is `Option<T>` (omit → skip the
    // predicate, a value → apply it), the same wrapper as a `(default)`/`type?` param.
    // `wrap_opt` picks the wrapper from the param.
    let wrap_opt = |base: String| -> String {
        if p.optional {
            // 2-state: strip a column-level `Option<…>` so a nullable column doesn't nest
            // `Option<Option<T>>`. Null-matching is a body concern.
            let inner = base
                .strip_prefix("Option<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or(&base);
            format!("Option<{inner}>")
        } else if (p.default.is_some() || p.ty.as_ref().is_some_and(|t| t.optional))
            && !base.starts_with("Option<")
        {
            format!("Option<{base}>")
        } else {
            base
        }
    };
    // An UpperCamel annotation names an *enum* when it resolves to one:
    // `status: Status` takes the enum's own generated type.
    if let Some(te) = &p.ty {
        if let BaseType::Model(name) = &te.base {
            if schema.enum_(&name.node).is_some() {
                return wrap_opt(wrap(&name.node, false, te.many));
            }
            // A shape-typed param — the row input of a `create … from $p`. Its
            // Rust type is the shape's own struct (`Vec<Shape>` for the bulk form),
            // reusing the same struct a query returning that shape emits.
            if schema.shapes.iter().any(|s| s.name == name.node) {
                return wrap_opt(wrap(&name.node, false, te.many));
            }
        }
    }
    let base = if let Some(entity) = param_entity(c, p) {
        let many = p.ty.as_ref().is_some_and(|t| t.many);
        wrap(&id_type(schema, &entity), false, many)
    } else {
        match &p.ty {
            Some(te) => wrap(base_type(&te.base), false, te.many),
            None => infer_param(schema, c.root, p),
        }
    };
    wrap_opt(base)
}

/// The entity a param identifies, if any: an explicit model annotation, else the
/// model the front end resolved it to (its query binding or mutation-body use).
/// `None` for a plain scalar param.
fn param_entity(c: &Callable, p: &Param) -> Option<String> {
    if let Some(te) = &p.ty {
        if let BaseType::Model(name) = &te.base {
            return Some(name.node.clone());
        }
    }
    c.param_entities.get(p.name.node.as_str()).cloned()
}

/// Infer an untyped param's type from how it filters: an `-> edge` or same-name
/// relation param is the FK (`Uuid`); an `op col` binding or same-name scalar
/// takes that column's type. Falls back to `Uuid` with no model to resolve
/// against (sema would already have flagged an unresolved param).
fn infer_param(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> String {
    let field = match &p.binding {
        Some(ParamBinding::Edge(edge)) => &edge.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None => &p.name.node,
    };
    reach_type(schema, root, &[field])
}
