//! Client codegen (M4): a `CheckedSchema` -> a typed client module (`based gen
//! client`). Rust is the first (and, per the manifest default, only) target.
//!
//! ## The closed RPC surface (calling.md)
//! Clients call the pre-defined query/mutation signatures only — they never write or
//! send the DSL, and the wire carries *arguments*, not queries. So each signature
//! generates exactly three things:
//!   1. a typed **input** struct (fields from the signature params),
//!   2. a typed **output** type (from `-> Output`: a shape struct, a bare-model
//!      struct, or the pagination envelope `Page<T>`),
//!   3. one **wire route** (`POST /q/<name>` for a query, `POST /m/<name>` for a
//!      mutation) plus a `Client` method that posts the input and decodes the output.
//!
//! ## What we emit vs. what the runtime owns
//! Transport is abstract: the generated `Client<T>` is generic over a `Transport`
//! trait (post JSON to a route, decode JSON back). The actual HTTP/driver lives in
//! the runtime crate (not started), so codegen emits the *typed surface* — input
//! types, output types, routes, and method bodies that delegate to `Transport` —
//! without inventing an HTTP stack. This keeps the generated code honest.
//!
//! ## Type mapping (mirrors the DDL side, D10)
//! Primitives map through readable aliases so the generated structs read
//! semantically: `Uuid`/`Timestamp`/`Date` alias `String`, `Json` aliases
//! `serde_json::Value`. A relation param (or a shape field reaching a relation's FK)
//! is a `Uuid` — the wire carries the id, per D1. `optional` -> `Option<T>`, a
//! to-many scalar -> `Vec<T>`.
//!
//! ## Deferred (documented, not silently wrong)
//! - Nested shape sub-objects (`field { … }`) are skipped in the output struct, same
//!   as the read side (they need JSON aggregation; PLAN M3).
//! - A `sql`…`` shape field has no statically known type, so it maps to `Json`.
//! - The keyset cursor is an opaque `Option<String>` in `Page<T>`; its encoding is a
//!   runtime concern (pagination.md).

use based_ast::*;
use based_sema::{CheckedSchema, MemberKind, RModel, RQuery};

/// The client compile target (manifest `client`). Rust is first and only for now;
/// the enum exists so the entry point can branch when a second target lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientTarget {
    Rust,
}

impl ClientTarget {
    /// Parse the manifest `client` string. Unknown values fall back to Rust (the
    /// documented default) rather than failing — target selection is not an error.
    pub fn parse(s: &str) -> ClientTarget {
        match s {
            "rust" => ClientTarget::Rust,
            _ => ClientTarget::Rust,
        }
    }
}

/// Render the whole schema as a typed client module for `target`.
pub fn client(schema: &CheckedSchema, decls: &[Decl], target: ClientTarget) -> String {
    let ClientTarget::Rust = target;
    rust::render(schema, decls)
}

// ---------- the resolved surface a callable contributes --------------------

/// What a single query/mutation lowers to on the client: an input struct, a wire
/// route, an output type, and the method that ties them together.
struct Callable<'a> {
    /// signature name (also the method name and the route tail) — already snake_case.
    name: &'a str,
    /// `/q/<name>` for a query, `/m/<name>` for a mutation.
    route: String,
    params: &'a [Param],
    /// model the params resolve against (query target / mutation return model);
    /// `None` when it could not be resolved (a mutation with no model return).
    root: Option<&'a RModel>,
    /// the concrete output type expression, e.g. `Vec<OrderCard>` or `Page<Product>`.
    output: String,
    /// the output *struct* to emit (name + fields), deduped across callables.
    out_struct: OutStruct,
}

/// A named output struct: a shape projection or a bare-model row.
struct OutStruct {
    name: String,
    fields: Vec<(String, String)>, // (field name, rust type)
}

// ---------- Rust target ----------------------------------------------------

mod rust {
    use super::*;

    pub(super) fn render(schema: &CheckedSchema, decls: &[Decl]) -> String {
        let callables = collect(schema, decls);

        let mut out = String::new();
        out.push_str(PREAMBLE);

        // Output structs first (deduped by name; a shape shared by two queries is one
        // struct). Emitted in first-seen order for deterministic output.
        out.push_str("\n// ---------- output types ----------\n");
        let mut seen: Vec<&str> = Vec::new();
        for c in &callables {
            if seen.contains(&c.out_struct.name.as_str()) {
                continue;
            }
            seen.push(&c.out_struct.name);
            out.push('\n');
            out.push_str(&render_struct(&c.out_struct.name, &c.out_struct.fields));
        }

        // Input structs + routes, in declaration order.
        out.push_str("\n// ---------- inputs + routes ----------\n");
        for c in &callables {
            out.push('\n');
            let fields = input_fields(schema, c);
            out.push_str(&render_struct(&input_name(c.name), &fields));
            out.push_str(&format!(
                "/// Wire route for `{}`.\npub const {}: &str = \"{}\";\n",
                c.name,
                route_const(c.name),
                c.route
            ));
        }

        // The client: one typed method per callable, each posting to its route.
        out.push_str("\n// ---------- client ----------\n\n");
        out.push_str("impl<T: Transport> Client<T> {\n");
        for c in &callables {
            out.push_str(&render_method(c));
        }
        out.push_str("}\n");
        out
    }

    /// Build the callable descriptors from the checked schema + AST.
    fn collect<'a>(schema: &'a CheckedSchema, decls: &'a [Decl]) -> Vec<Callable<'a>> {
        let queries: std::collections::HashMap<&str, &RQuery> = schema
            .queries
            .iter()
            .map(|q| (q.name.as_str(), q))
            .collect();

        let mut out = Vec::new();
        for decl in decls {
            match decl {
                Decl::Query(q) => {
                    let Some(rq) = queries.get(q.name.node.as_str()) else {
                        continue;
                    };
                    let root = schema.model(&rq.target);
                    let os = out_struct(schema, decls, &q.ret, root);
                    out.push(Callable {
                        name: &q.name.node,
                        route: format!("/q/{}", q.name.node),
                        params: &q.params,
                        root,
                        output: query_output(rq, &os.name),
                        out_struct: os,
                    });
                }
                Decl::Mutation(m) => {
                    let root = schema.model(&m.ret.ty.node).or_else(|| {
                        // A shape return: resolve the model it projects from.
                        schema
                            .shapes
                            .iter()
                            .find(|s| s.name == m.ret.ty.node)
                            .and_then(|s| schema.model(&s.from))
                    });
                    let os = out_struct(schema, decls, &m.ret, root);
                    let output = if m.ret.many {
                        format!("Vec<{}>", os.name)
                    } else {
                        os.name.clone()
                    };
                    out.push(Callable {
                        name: &m.name.node,
                        route: format!("/m/{}", m.name.node),
                        params: &m.params,
                        root,
                        output,
                        out_struct: os,
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// A query's return wrapper: paginated -> `Page<T>`, many -> `Vec<T>`, single
    /// -> `Option<T>` (a `get` may match nothing). (calling.md pagination envelope.)
    fn query_output(rq: &RQuery, ty: &str) -> String {
        if rq.paginated {
            format!("Page<{ty}>")
        } else if rq.many {
            format!("Vec<{ty}>")
        } else {
            format!("Option<{ty}>")
        }
    }

    // ---------- output structs ---------------------------------------------

    /// Resolve a return type to the struct we emit for it. A shape projects its body;
    /// a bare model (or `full`) projects every stored column.
    fn out_struct(
        schema: &CheckedSchema,
        decls: &[Decl],
        ret: &RetType,
        root: Option<&RModel>,
    ) -> OutStruct {
        let name = ret.ty.node.as_str();
        // A declared shape: its struct is the projected body against the shape model.
        if name != "full" {
            if let Some(shape) = find_shape(decls, name) {
                let model = schema.model(&shape.from.node);
                return OutStruct {
                    name: name.to_string(),
                    fields: shape_fields(schema, &shape.body, model),
                };
            }
        }
        // `full` or a bare model: every stored column of the resolved model.
        match root {
            Some(m) => OutStruct {
                name: m.name.clone(),
                fields: model_fields(m),
            },
            // Unresolvable (sema would have flagged it) — an empty struct keeps the
            // emitted module compiling rather than referencing a missing type.
            None => OutStruct {
                name: pascal(name),
                fields: Vec::new(),
            },
        }
    }

    /// Project a shape body into `(field, type)` pairs. Nested sub-objects are
    /// skipped (deferred, like the SQL side); a `sql`…`` field maps to `Json`.
    fn shape_fields(
        schema: &CheckedSchema,
        body: &[ShapeField],
        model: Option<&RModel>,
    ) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        for f in body {
            match f {
                ShapeField::Bare(id) => {
                    fields.push((id.node.clone(), reach_type(schema, model, &[&id.node])));
                }
                ShapeField::Rename { out, value } => match value {
                    ShapeValue::Path(p) => {
                        let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
                        fields.push((out.node.clone(), reach_type(schema, model, &segs)));
                    }
                    // A raw SQL expression has no statically known type -> `Json`.
                    ShapeValue::Raw(_) => fields.push((out.node.clone(), "Json".to_string())),
                },
                ShapeField::Nest { .. } => {}
            }
        }
        fields
    }

    /// Every stored column of a bare-model return: scalars by their type, forward
    /// FKs as a `Uuid` under the relation field name (matching the SELECT alias).
    /// Inverse edges store nothing, so they are omitted.
    fn model_fields(model: &RModel) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        for mem in &model.members {
            match &mem.kind {
                MemberKind::Scalar {
                    ty, optional, many, ..
                } => fields.push((mem.name.clone(), wrap(primitive(*ty), *optional, *many))),
                MemberKind::Forward { optional, .. } => {
                    fields.push((mem.name.clone(), wrap("Uuid", *optional, false)))
                }
                MemberKind::Inverse { .. } => {}
            }
        }
        fields
    }

    // ---------- input structs ----------------------------------------------

    /// The input fields for a callable: one per signature param, typed from its
    /// explicit annotation or inferred from the column it maps to.
    fn input_fields(schema: &CheckedSchema, c: &Callable) -> Vec<(String, String)> {
        c.params
            .iter()
            .map(|p| (p.name.node.clone(), param_type(schema, c.root, p)))
            .collect()
    }

    /// A param's Rust type. Explicit annotation wins (a model type -> `Uuid`, the FK
    /// the wire carries, D1); otherwise infer from the bound/same-named column. A
    /// param with a default (or an optional annotation) becomes `Option<T>` — the
    /// client may omit it and let the engine apply the default (calling.md).
    fn param_type(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> String {
        let optional = p.default.is_some() || p.ty.as_ref().is_some_and(|t| t.optional);
        let base = match &p.ty {
            Some(te) => wrap(base_type(&te.base), false, te.many),
            None => infer_param(schema, root, p),
        };
        if optional && !base.starts_with("Option<") {
            format!("Option<{base}>")
        } else {
            base
        }
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

    // ---------- type resolution --------------------------------------------

    /// Resolve a dotted field path against `model` to a Rust type. A scalar terminal
    /// is its mapped primitive (carrying `optional`/`many`); a relation terminal is
    /// the FK `Uuid`; intermediate relation hops walk to the target model. Unknown
    /// paths (sema already flagged) fall back to `Json` so the module still compiles.
    fn reach_type(schema: &CheckedSchema, model: Option<&RModel>, path: &[&str]) -> String {
        let Some(mut cur) = model else {
            return "Uuid".to_string();
        };
        let n = path.len();
        for (i, seg) in path.iter().enumerate() {
            let last = i + 1 == n;
            match cur.member(seg).map(|m| &m.kind) {
                Some(MemberKind::Scalar {
                    ty, optional, many, ..
                }) => return wrap(primitive(*ty), *optional, *many),
                Some(MemberKind::Forward {
                    target, optional, ..
                }) => {
                    if last {
                        return wrap("Uuid", *optional, false);
                    }
                    match schema.model(target) {
                        Some(m) => cur = m,
                        None => return "Json".to_string(),
                    }
                }
                Some(MemberKind::Inverse { target, .. }) => {
                    if last {
                        // Terminal to-many reach: a collection of ids.
                        return "Vec<Uuid>".to_string();
                    }
                    match schema.model(target) {
                        Some(m) => cur = m,
                        None => return "Json".to_string(),
                    }
                }
                None => return "Json".to_string(),
            }
        }
        "Json".to_string()
    }

    /// Wrap a base type: to-many -> `Vec<base>`, then optional -> `Option<…>`.
    fn wrap(base: &str, optional: bool, many: bool) -> String {
        let inner = if many {
            format!("Vec<{base}>")
        } else {
            base.to_string()
        };
        if optional {
            format!("Option<{inner}>")
        } else {
            inner
        }
    }

    /// A primitive type name as its Rust alias (see the module type-mapping table).
    fn primitive(p: Primitive) -> &'static str {
        match p {
            Primitive::Text => "String",
            Primitive::Int => "i64",
            Primitive::Bool => "bool",
            Primitive::Timestamp => "Timestamp",
            Primitive::Date => "Date",
            Primitive::Json => "Json",
            Primitive::Uuid | Primitive::Id => "Uuid",
        }
    }

    /// A param/field base type: a primitive by its alias, a model reference as the
    /// `Uuid` FK the wire carries (D1).
    fn base_type(b: &BaseType) -> &'static str {
        match b {
            BaseType::Primitive(p) => primitive(*p),
            BaseType::Model(_) => "Uuid",
        }
    }

    // ---------- rendering helpers ------------------------------------------

    /// A `#[derive(...)] pub struct Name { pub field: Type, … }` block. An empty body
    /// renders as a unit-like struct (a callable with no params posts `{}`).
    fn render_struct(name: &str, fields: &[(String, String)]) -> String {
        let mut s = format!("#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {name}");
        if fields.is_empty() {
            s.push_str(";\n");
            return s;
        }
        s.push_str(" {\n");
        for (f, ty) in fields {
            s.push_str(&format!("    pub {}: {ty},\n", field_ident(f)));
        }
        s.push_str("}\n");
        s
    }

    /// One typed client method: `POST` the input to the route, decode the output.
    fn render_method(c: &Callable) -> String {
        format!(
            "    /// `POST {route}`\n    pub fn {name}(&self, input: {input}) -> Result<{output}, ClientError> {{\n        self.transport.call({konst}, &input)\n    }}\n",
            route = c.route,
            name = field_ident(c.name),
            input = input_name(c.name),
            output = c.output,
            konst = route_const(c.name),
        )
    }

    fn input_name(name: &str) -> String {
        format!("{}Input", pascal(name))
    }

    fn route_const(name: &str) -> String {
        format!("{}_ROUTE", name.to_uppercase())
    }

    /// snake_case / lower name -> UpperCamel (`order_by_id` -> `OrderById`). Already
    /// UpperCamel shape/model names pass through unchanged.
    fn pascal(name: &str) -> String {
        name.split('_')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut cs = s.chars();
                match cs.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            })
            .collect()
    }

    /// Escape a field/method name that collides with a Rust keyword (`type` ->
    /// `r#type`). The DSL's identifier set is broader than Rust's reserved words.
    fn field_ident(name: &str) -> String {
        const KEYWORDS: &[&str] = &[
            "type", "match", "move", "ref", "box", "fn", "let", "mut", "impl", "trait", "struct",
            "enum", "self", "crate", "super", "async", "await", "dyn", "loop", "where",
        ];
        if KEYWORDS.contains(&name) {
            format!("r#{name}")
        } else {
            name.to_string()
        }
    }

    /// Find a shape by name in the AST (its body drives the output struct). Names are
    /// unique across shapes except `full`, which this never looks up (handled above).
    fn find_shape<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Shape> {
        decls.iter().find_map(|d| match d {
            Decl::Shape(s) if s.name.node == name => Some(s),
            _ => None,
        })
    }

    /// The fixed module prelude: header, type aliases, the pagination envelope, the
    /// error type, and the abstract transport the runtime later supplies.
    const PREAMBLE: &str = r#"// Generated by `based gen client` (target: rust). Do not edit by hand.
//
// The closed RPC surface (calling.md): one input type, one output type, and one
// route per signature. Transport is abstract — implement `Transport` to post JSON
// to a route and decode the reply; the runtime supplies the concrete HTTP client.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// Semantic aliases for the wire types (mirrors the DDL mapping, D10).
pub type Uuid = String;
pub type Timestamp = String;
pub type Date = String;
pub type Json = serde_json::Value;

/// Pagination envelope (calling.md): a paginated query returns rows + an opaque
/// cursor, never a bare array. Next page = the same call carrying `cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub cursor: Option<String>,
}

/// An error from a client call (transport failure or decode failure). The concrete
/// `Transport` decides how to populate it.
#[derive(Debug, Clone)]
pub struct ClientError(pub String);

/// Post a typed input to a route and decode the typed output. Implemented by the
/// runtime's HTTP client; codegen only depends on this shape.
pub trait Transport {
    fn call<I, O>(&self, route: &str, input: &I) -> Result<O, ClientError>
    where
        I: Serialize,
        O: serde::de::DeserializeOwned;
}

/// The generated client, generic over a `Transport`.
pub struct Client<T> {
    pub transport: T,
}
"#;
}
