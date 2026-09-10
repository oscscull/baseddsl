use super::*;

/// A short JSON-kind label for a boundary error message.
pub(crate) fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}


/// Named bind values gathered from the validated request; `bind` pulls from it in
/// SQL placeholder order. Carries the target `dialect` so the positional rewrite emits
/// the right placeholder form (`?` vs `$n`).
pub(crate) struct Env {
    pub(crate) dialect: based_codegen::Dialect,
    pub(crate) values: std::collections::HashMap<String, SqlValue>,
}

impl Env {
    pub(crate) fn new(dialect: based_codegen::Dialect) -> Self {
        Self {
            dialect,
            values: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, name: String, v: SqlValue) {
        self.values.insert(name, v);
    }

    /// A clone of the bound values — the seed a deferred (serial) re-select binds from.
    pub(crate) fn snapshot(&self) -> std::collections::HashMap<String, SqlValue> {
        self.values.clone()
    }

    /// Rewrite one statement to positional form, resolving each `:name` from the
    /// environment. An unresolved name is an internal invariant break, not a user
    /// error (every declared bind was inserted above).
    pub(crate) fn bind(&self, sql: &str) -> Result<Stmt, PlanError> {
        let (sql, params) = to_positional(sql, self.dialect, |name| {
            self.values.get(name).cloned().map(SqlValue::expand)
        })
        .map_err(PlanError::UnboundPlaceholder)?;
        Ok(Stmt { sql, params })
    }
}


/// Bind one signature param: use the supplied arg (coerced to the resolved family),
/// or its default, or `null` if optional — else it is missing.
pub(crate) fn bind_param(
    schema: &CheckedSchema,
    p: &Param,
    family: Family,
    optional: bool,
    req: &Request,
) -> Result<SqlValue, PlanError> {
    if let Some(v) = req.args.get(&p.name.node) {
        coerce(v, family, optional).map_err(|e| bad_arg(&p.name.node, e))
    } else {
        if let Some(dv) = &p.default {
            return Ok(default_value(schema, p, dv, family));
        }
        if optional {
            return Ok(SqlValue::Null);
        }
        Err(PlanError::MissingArg(p.name.node.clone()))
    }
}

/// Coerce an array-typed arg (`col in $arr`) into a bind [`SqlValue::List`]: the arg must be a
/// JSON array, each element coerced against the column's scalar `family`. A non-array arg is a
/// boundary error; a `null` element is rejected (an `IN (NULL)` never matches, so a null in the
/// list is meaningless). An empty array is valid — it lowers to `col IN (NULL)` (matches
/// nothing) at expansion time.
pub(crate) fn coerce_list(v: &serde_json::Value, family: Family) -> Result<SqlValue, CoerceError> {
    match v {
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(coerce(it, family, false)?);
            }
            Ok(SqlValue::List(out))
        }
        other => Err(CoerceError {
            expected: family,
            got: format!("{} (expected an array)", json_kind(other)),
        }),
    }
}

/// The coercion family + nullability of a query param. An explicit annotation wins;
/// an untyped param takes the family of the column it binds against (its `-> edge` /
/// `op col` binding, else the same-named member of the target model) — the same
/// inference the generated client types the input by. Unresolvable (raw SQL) params
/// stay `Any`: shape-coerced, a plain text bind.
pub(crate) fn query_param_family(
    schema: &CheckedSchema,
    root: Option<&RModel>,
    p: &Param,
    entity: Option<&str>,
) -> (Family, bool) {
    // A param that identifies a model (its annotation, or its binding / `= $param`
    // comparison against an FK / id) carries that model's key family — serial → int, not the
    // project-default uuid. This is the single resolution the client/OpenAPI also use.
    if let Some(e) = entity {
        let optional = p.ty.as_ref().is_some_and(|t| t.optional) || p.default.is_some();
        return (target_key_family(schema, e), optional);
    }
    if let Some(t) = &p.ty {
        let family = match &t.base {
            BaseType::Primitive(prim) => Family::of(*prim),
            // An opaque `raw(…)` value binds as plain text (sema keeps it out of
            // params, so this is only reached through a hand-built plan).
            BaseType::Raw(_) => Family::Text,
            // An UpperCamel annotation that is not a model reference: an enum param carries
            // the enum's wire value (its storage family).
            BaseType::Model(name) => enum_or_uuid(schema, &name.node),
        };
        return (family, t.optional || p.default.is_some());
    }
    let field = binding_field(p);
    let family = root
        .and_then(|m| member_family(schema, m, &[field]))
        .unwrap_or(Family::Any);
    (family, p.default.is_some())
}

/// The coercion family + nullability of a mutation param: an explicit annotation wins;
/// an untyped param takes the family of the first column its `$name` fills or filters
/// in the write body. Unresolvable stays `Any`.
pub(crate) fn mutation_param_family(
    compiled: &Compiled,
    ast: &Mutation,
    p: &Param,
    entity: Option<&str>,
) -> (Family, bool) {
    // A param identifying a model (annotation, or a `col = $param` write-body use against an
    // FK / id) carries that model's key family — serial → int, not the project-default uuid.
    if let Some(e) = entity {
        let optional = p.ty.as_ref().is_some_and(|t| t.optional) || p.default.is_some();
        return (target_key_family(&compiled.schema, e), optional);
    }
    if let Some(t) = &p.ty {
        let family = match &t.base {
            BaseType::Primitive(prim) => Family::of(*prim),
            BaseType::Raw(_) => Family::Text,
            BaseType::Model(name) => enum_or_uuid(&compiled.schema, &name.node),
        };
        return (family, t.optional || p.default.is_some());
    }
    let family = param_use_in_stmts(compiled, &ast.body, &p.name.node).unwrap_or(Family::Any);
    (family, p.default.is_some())
}

/// An UpperCamel param annotation's family: the enum's storage family when the name
/// resolves to an enum (text for a string enum, int for an int one), else a relation
/// target's key (uuid).
pub(crate) fn enum_or_uuid(schema: &CheckedSchema, name: &str) -> Family {
    match schema.enum_(name) {
        Some(e) if e.is_int() => Family::Int,
        Some(_) => Family::Text,
        None => Family::Uuid,
    }
}

/// The field a query param binds against: its `-> edge` / `op col` binding, else its
/// own name.
pub(crate) fn binding_field(p: &Param) -> &str {
    use based_ast::ParamBinding;
    match &p.binding {
        Some(ParamBinding::Edge(e)) => &e.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None => &p.name.node,
    }
}

/// The family of the member a dotted path terminates in: a scalar is its primitive,
/// a relation terminal is the target's key (mirroring the target PK — a uuid/ulid string,
/// or a serial integer). `None` when unresolved.
pub(crate) fn member_family(schema: &CheckedSchema, model: &RModel, path: &[&str]) -> Option<Family> {
    let mut cur = model;
    let n = path.len();
    for (i, seg) in path.iter().enumerate() {
        let last = i + 1 == n;
        match &cur.member(seg)?.kind {
            MemberKind::Scalar { ty, .. } => return Some(Family::of(*ty)),
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return Some(target_key_family(schema, target));
                }
                cur = schema.model(target)?;
            }
        }
    }
    None
}

/// The coercion family of a model's primary key — the value an inbound relation FK
/// carries. Mirrors the target's `id` type (serial → int, uuid/ulid → uuid string);
/// defaults to uuid when the target or its id is unresolved.
pub(crate) fn target_key_family(schema: &CheckedSchema, target: &str) -> Family {
    match schema
        .model(target)
        .and_then(RModel::pk_member)
        .map(|m| &m.kind)
    {
        Some(MemberKind::Scalar { ty, .. }) => Family::of(*ty),
        _ => Family::Uuid,
    }
}

/// Bind one `$ctx.<field>` requirement into the value environment as `ctx_<field>`. An
/// **optional** field (`$ctx.field?`, binds SQL NULL when absent (missing
/// key or JSON null): the codegen lowers `=`/`!=` against it to null-safe (in)equality, so an
/// absent field matches the rows whose own column is unset rather than widening the filter. A
/// **required** field binds the value alone and errors when absent (`bind_ctx`).
pub(crate) fn bind_ctx_into(
    env: &mut Env,
    schema: &CheckedSchema,
    c: &CtxReq,
    req: &Request,
) -> Result<(), PlanError> {
    let key = format!("ctx_{}", c.field);
    if c.optional {
        let present = matches!(req.ctx.get(&c.field), Some(v) if !v.is_null());
        let value = if present {
            bind_ctx(schema, c, req)?
        } else {
            SqlValue::Null
        };
        env.insert(key, value);
    } else {
        env.insert(key, bind_ctx(schema, c, req)?);
    }
    Ok(())
}

/// Bind one `$ctx.<field>` requirement from the request context. Always required —
/// the callable cannot run without the context it reads.
pub(crate) fn bind_ctx(schema: &CheckedSchema, c: &CtxReq, req: &Request) -> Result<SqlValue, PlanError> {
    let family = match &c.ty {
        CtxField::Scalar(prim) => Family::of(*prim),
        // A relation-typed context field carries the model's key — mirroring the target
        // PK (serial → int, uuid/ulid → string).
        CtxField::Relation(target) => target_key_family(schema, target),
    };
    match req.ctx.get(&c.field) {
        Some(v) => coerce(v, family, false).map_err(|e| PlanError::BadCtx {
            field: c.field.clone(),
            expected: e.expected,
            got: e.got,
        }),
        None => Err(PlanError::MissingCtx(c.field.clone())),
    }
}

/// Bind the `:offset` of an offset page. The client sends `offset`; absence means
/// the first page (offset 0), never an error (the default is safe).
pub(crate) fn bind_offset(req: &Request) -> Result<SqlValue, PlanError> {
    match req.args.get("offset") {
        Some(v) => coerce(v, Family::Int, false).map_err(|e| bad_arg("offset", e)),
        None => Ok(SqlValue::Int(0)),
    }
}

/// Bind a keyset page's cursor placeholders (`:keyset_active` + `:keyset_0..n`). The
/// caller sends the opaque `cursor` arg; absence is the first page (`:keyset_active =
/// 0`, the comparison a no-op, the value placeholders NULL — never consulted). A
/// present cursor is decoded + validated into one value per sort key (`cursor.rs`),
/// each re-bound as its sort column's own primitive (so a typed driver binds the same
/// type the row carried); a bad cursor is a `BadCursor` boundary error, not a silent
/// empty page.
pub(crate) fn bind_cursor(env: &mut Env, req: &Request, prims: &[Primitive]) -> Result<(), PlanError> {
    match req.args.get("cursor").filter(|v| !v.is_null()) {
        Some(serde_json::Value::String(s)) => {
            let vals =
                crate::cursor::decode(s, prims.len()).map_err(|e| PlanError::BadCursor(e.0))?;
            env.insert("keyset_active".into(), SqlValue::Int(1));
            for (i, (v, prim)) in vals.iter().zip(prims).enumerate() {
                // A null sort-key value (a nullable sort column) stays NULL; anything
                // else must fit the column's family — a cursor value of the wrong
                // shape is a tampered/foreign cursor, the same boundary error.
                let bound = coerce(v, Family::of(*prim), true).map_err(|e| {
                    PlanError::BadCursor(format!("expected {}", e.expected.label()))
                })?;
                env.insert(format!("keyset_{i}"), bound);
            }
        }
        Some(_) => return Err(PlanError::BadCursor("cursor must be a string".into())),
        None => {
            env.insert("keyset_active".into(), SqlValue::Int(0));
            for i in 0..prims.len() {
                env.insert(format!("keyset_{i}"), SqlValue::Null);
            }
        }
    }
    Ok(())
}


pub(crate) fn bad_arg(name: &str, e: CoerceError) -> PlanError {
    PlanError::BadArg {
        name: name.to_string(),
        expected: e.expected,
        got: e.got,
    }
}

/// A literal default → its bound value, in the param's resolved family (so a string
/// default on a `timestamp` column still binds typed). A `now()` default has no
/// request-time value (it is a write-time engine concern) → `Null` here; query params
/// default to literals in practice.
pub(crate) fn default_value(schema: &CheckedSchema, p: &Param, dv: &DefaultVal, family: Family) -> SqlValue {
    match dv {
        DefaultVal::Lit(Literal::Str(s)) => string_in_family(s.clone(), family),
        DefaultVal::Lit(Literal::Int(i)) => SqlValue::Int(*i),
        // A fractional literal stays exact text for a decimal column; a float default
        // parses to its number.
        DefaultVal::Lit(Literal::Decimal(s)) => match family {
            Family::Float => SqlValue::Float(s.parse().unwrap_or(0.0)),
            _ => SqlValue::Decimal(s.clone()),
        },
        DefaultVal::Lit(Literal::Bool(b)) => SqlValue::Bool(*b),
        DefaultVal::Lit(Literal::Null) => SqlValue::Null,
        DefaultVal::Func(_) => SqlValue::Null,
        // An enum default binds the variant's WIRE value (an int-enum discriminant, or
        // a string enum's possibly-renamed value) — never the variant's source name.
        DefaultVal::Variant(v) => match variant_wire(schema, p, &v.node) {
            Some(based_sema::EnumValue::Int(i)) => SqlValue::Int(*i),
            Some(based_sema::EnumValue::Str(s)) => SqlValue::Text(s.clone()),
            None => SqlValue::Text(v.node.clone()),
        },
    }
}

/// The wire value of a variant default: resolve the param's enum annotation
/// (`status: Status = open`) and look the variant up in it.
pub(crate) fn variant_wire<'a>(
    schema: &'a CheckedSchema,
    p: &Param,
    variant: &str,
) -> Option<&'a based_sema::EnumValue> {
    let ty = p.ty.as_ref()?;
    let BaseType::Model(name) = &ty.base else {
        return None;
    };
    schema.enum_(&name.node)?.wire_of(variant)
}

/// Wrap a string literal in the typed variant its family calls for.
pub(crate) fn string_in_family(s: String, family: Family) -> SqlValue {
    match family {
        Family::Uuid => SqlValue::Uuid(s),
        Family::Timestamp => SqlValue::Timestamp(s),
        Family::Date => SqlValue::Date(s),
        Family::Time => SqlValue::Time(s),
        Family::Decimal => SqlValue::Decimal(s),
        Family::Bytes => SqlValue::Bytes(s),
        _ => SqlValue::Text(s),
    }
}
