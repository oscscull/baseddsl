//! Client codegen: a `CheckedSchema` -> a typed Rust client module (`based gen
//! client`). Per signature it emits a typed input struct, a typed output type (shape
//! struct, bare-model struct, or the `Page<T>` pagination envelope), and one wire route
//! plus a `Client` method that posts the input and decodes the output.
//!
//! Transport is abstract: `Client<T>` is generic over a `Transport` trait (post JSON to
//! a route, decode JSON back), which the runtime crate implements. Entity ids map to a
//! phantom-typed `Id<E>` newtype and the keyset cursor to an opaque `Cursor`, both
//! `#[serde(transparent)]` so the wire stays a plain string. `$ctx` is carried out of
//! band as request context. Shape projections nest to matching structs.
//!
//! When [`ClientOptions::embedded`] is set, the module also emits an in-process bridge
//! over `based_runtime::Engine` (an `Embedded` transport plus an `embedded(&engine)`
//! constructor), giving an embedding consumer a working `Client` with no bridge code.
//! Opt-in so a pure-wire client need not depend on based-runtime.

use based_ast::*;
use based_sema::{CheckedSchema, CtxField, CtxReq, MemberKind, RModel, RQuery};

/// The client compile target (manifest `client`). Rust is the only target; the
/// enum exists so the entry point can branch when a second target lands.
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

/// Emit options beyond the target language.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientOptions {
    /// Also emit the in-process **embedded bridge** — an `Embedded` `Transport` over
    /// `based_runtime::Engine` plus an `embedded(&engine)` constructor. Off by default so
    /// a pure-wire/HTTP client need not depend on based-runtime. An embedding consumer
    /// (the quickstarts, `tests/embed.rs`) turns it on to get a working `Client` with no
    /// hand-written bridge.
    pub embedded: bool,
}

/// Render the whole schema as a typed *wire* client module for `target` (no embedded
/// bridge). The socket-free embed path uses [`client_with`] with
/// [`ClientOptions::embedded`] instead.
pub fn client(schema: &CheckedSchema, decls: &[Decl], target: ClientTarget) -> String {
    client_with(schema, decls, target, ClientOptions::default())
}

/// Render the whole schema as a typed client module for `target`, honoring `opts` (e.g.
/// [`ClientOptions::embedded`] to append the in-process bridge over `based_runtime::Engine`).
pub fn client_with(
    schema: &CheckedSchema,
    decls: &[Decl],
    target: ClientTarget,
    opts: ClientOptions,
) -> String {
    let ClientTarget::Rust = target;
    rust::render(schema, decls, opts)
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
    /// the `$ctx.<field>`s this callable requires, inferred per callable.
    /// Empty for a public callable (no context); non-empty callables get a typed
    /// `<Name>Ctx` struct the method takes and the `Transport` carries.
    ctx_requires: &'a [CtxReq],
    /// how this callable paginates, so the input struct carries the right page
    /// control: a keyset page a `cursor`, an offset page an `offset`.
    page: PageInput,
    /// Params that resolve to an **entity id** → the model they identify (a Forward FK's
    /// target, or the model's own `id`). Drives the `Id<entity::M>` param type; a param
    /// absent here (and not model-annotated) is a plain scalar.
    param_entities: std::collections::HashMap<String, String>,
}

/// How a callable paginates, driving its extra input field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PageInput {
    /// Not paginated (a `get`, or a `list` with no `page`) — no page-control input.
    None,
    /// Keyset (`page` without `offset`): an opaque `cursor: Option<Cursor>` (absent =
    /// the first page); the response's `Page.cursor` is fed straight back for the next.
    Keyset,
    /// Explicit offset (`page … offset`): an `offset: Option<i64>` (absent = offset 0).
    Offset,
}

/// A named output struct: a shape projection or a bare-model row.
struct OutStruct {
    name: String,
    fields: Vec<(String, String)>, // (field name, rust type)
    /// Auxiliary structs for to-one nested sub-objects (`buyer { … }`), each the
    /// projection of one nested relation, emitted alongside the parent and referenced by
    /// the parent's field type. Empty for a flat shape.
    nested: Vec<OutStruct>,
}

// ---------- Rust target ----------------------------------------------------

mod rust {
    use super::*;

    pub(super) fn render(schema: &CheckedSchema, decls: &[Decl], opts: ClientOptions) -> String {
        let callables = collect(schema, decls);

        let mut out = String::new();
        out.push_str(PREAMBLE);

        // Entity markers — one phantom tag per model, so `Id<entity::User>` and
        // `Id<entity::Org>` are distinct types (the tags are types only, never values).
        out.push_str("\n/// Phantom entity tags for `Id<entity::M>` (types only, never constructed).\npub mod entity {\n");
        for m in &schema.models {
            out.push_str(&format!("    pub enum {} {{}}\n", m.name));
        }
        out.push_str("}\n");

        // Output structs first (deduped by name; a shape shared by two queries is one
        // struct). Emitted in first-seen order for deterministic output.
        out.push_str("\n// ---------- output types ----------\n");
        let mut seen: Vec<String> = Vec::new();
        for c in &callables {
            emit_struct(&mut out, &c.out_struct, &mut seen);
        }

        // Input structs (+ the per-callable `Ctx` struct, when the callable needs
        // context) + routes, in declaration order.
        out.push_str("\n// ---------- inputs + routes ----------\n");
        for c in &callables {
            out.push('\n');
            let fields = input_fields(schema, c);
            out.push_str(&render_struct(&input_name(c.name), &fields));
            // A callable that reads `$ctx.<field>`s gets a typed context struct the
            // method takes; a public callable (no requirements) takes `()`.
            if !c.ctx_requires.is_empty() {
                out.push_str(&render_struct(
                    &ctx_name(c.name),
                    &ctx_fields(c.ctx_requires),
                ));
            }
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

        // Opt-in in-process bridge over `based_runtime::Engine`: a working client with no
        // hand-written `Transport` impl. Gated so the wire client stays free of a
        // based-runtime dependency.
        if opts.embedded {
            out.push_str(EMBEDDED);
        }
        out
    }

    /// Build the callable descriptors from the checked schema + AST.
    fn collect<'a>(schema: &'a CheckedSchema, decls: &'a [Decl]) -> Vec<Callable<'a>> {
        let queries: std::collections::HashMap<&str, &RQuery> = schema
            .queries
            .iter()
            .map(|q| (q.name.as_str(), q))
            .collect();
        let mutations: std::collections::HashMap<&str, &_> = schema
            .mutations
            .iter()
            .map(|m| (m.name.as_str(), m))
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
                        ctx_requires: &rq.ctx_requires,
                        page: page_input(q),
                        param_entities: query_param_entities(root, &q.params),
                    });
                }
                Decl::Mutation(m) => {
                    let Some(rm) = mutations.get(m.name.node.as_str()) else {
                        continue;
                    };
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
                        ctx_requires: &rm.ctx_requires,
                        page: PageInput::None,
                        param_entities: mutation_param_entities(schema, m),
                    });
                }
                _ => {}
            }
        }
        out
    }

    // ---------- entity-id resolution --------------------------------------

    /// The Rust type of an entity id: a phantom-typed `Id<entity::M>` newtype, distinct
    /// per model so ids of different entities can't be swapped.
    fn id_type(entity: &str) -> String {
        format!("Id<entity::{entity}>")
    }

    /// The model a member identifies as an id: a Forward FK's target, or the model's own
    /// `id` column (`Primitive::Id`). `None` for any other scalar or an inverse edge.
    fn member_entity(model: &RModel, field: &str) -> Option<String> {
        match model.member(field).map(|m| &m.kind)? {
            MemberKind::Forward { target, .. } => Some(target.clone()),
            MemberKind::Scalar {
                ty: Primitive::Id, ..
            } => Some(model.name.clone()),
            _ => None,
        }
    }

    /// Resolve each query param to the entity it identifies, from its binding (`-> edge`
    /// / `op col`) or same-named column on the target model. Params that identify no
    /// entity (plain scalars) are absent from the map.
    fn query_param_entities(
        root: Option<&RModel>,
        params: &[Param],
    ) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        let Some(root) = root else { return map };
        for p in params {
            let field = match &p.binding {
                Some(ParamBinding::Edge(e)) => e.node.as_str(),
                Some(ParamBinding::ColOp { col, .. }) => col.node.as_str(),
                None => p.name.node.as_str(),
            };
            if let Some(entity) = member_entity(root, field) {
                map.insert(p.name.node.clone(), entity);
            }
        }
        map
    }

    /// Resolve each mutation param to the entity it identifies, by walking the write
    /// body: a param assigned to a Forward FK / `id`, or compared against one in a
    /// `where`, identifies that member's model. This is the front end's own resolution
    /// (the same edges sema type-checks), surfaced instead of discarded.
    fn mutation_param_entities(
        schema: &CheckedSchema,
        m: &Mutation,
    ) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for stmt in &m.body {
            scan_write(schema, stmt, &mut map);
        }
        map
    }

    fn scan_write(
        schema: &CheckedSchema,
        stmt: &WriteStmt,
        map: &mut std::collections::HashMap<String, String>,
    ) {
        match stmt {
            WriteStmt::Create { model, assigns } => {
                let m = schema.model(&model.node);
                for a in assigns {
                    scan_assign(m, a, map);
                }
            }
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => {
                let m = schema.model(&model.node);
                for a in assigns {
                    scan_assign(m, a, map);
                }
                scan_pred(m, where_, map);
            }
            WriteStmt::Delete { model, where_ }
            | WriteStmt::Restore { model, where_ }
            | WriteStmt::HardDelete { model, where_ } => {
                scan_pred(schema.model(&model.node), where_, map);
            }
            WriteStmt::Tx(stmts) => {
                for s in stmts {
                    scan_write(schema, s, map);
                }
            }
            WriteStmt::Raw(_) => {}
        }
    }

    /// Record `col = $param` when `col` is a Forward FK / `id` on `model`.
    fn scan_assign(
        model: Option<&RModel>,
        a: &Assign,
        map: &mut std::collections::HashMap<String, String>,
    ) {
        if let Value::Param(pr) = &a.value {
            if pr.path.is_empty() {
                if let Some(entity) = model.and_then(|m| member_entity(m, &a.col.node)) {
                    map.insert(pr.name.node.clone(), entity);
                }
            }
        }
    }

    /// Record `col = $param` comparisons in a `where` where `col` is an id member.
    fn scan_pred(
        model: Option<&RModel>,
        pred: &Predicate,
        map: &mut std::collections::HashMap<String, String>,
    ) {
        match pred {
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                scan_pred(model, a, map);
                scan_pred(model, b, map);
            }
            Predicate::Not(p) => scan_pred(model, p, map),
            Predicate::Cmp {
                path,
                value: Value::Param(pr),
                ..
            } if path.segments.len() == 1 && pr.path.is_empty() => {
                if let Some(entity) = model.and_then(|m| member_entity(m, &path.segments[0].node)) {
                    map.insert(pr.name.node.clone(), entity);
                }
            }
            _ => {}
        }
    }

    /// A query's return wrapper: paginated -> `Page<T>`, many -> `Vec<T>`, single
    /// -> `Option<T>` (a `get` may match nothing).
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
                return build_struct(
                    schema,
                    decls,
                    name.to_string(),
                    &shape.body,
                    model,
                    &mut vec![name.to_string()],
                );
            }
        }
        // `full` or a bare model: every stored column of the resolved model.
        match root {
            Some(m) => OutStruct {
                name: m.name.clone(),
                fields: model_fields(m),
                nested: Vec::new(),
            },
            // Unresolvable (sema would have flagged it) — an empty struct keeps the
            // emitted module compiling rather than referencing a missing type.
            None => OutStruct {
                name: pascal(name),
                fields: Vec::new(),
                nested: Vec::new(),
            },
        }
    }

    /// Build one output struct from a shape body: `(field, type)` pairs plus the
    /// auxiliary structs for its to-one nested sub-objects. A `sql`…`` field maps to
    /// `Json`; a to-one nest (`buyer { … }`) becomes a nested struct named
    /// `<Parent><Field>` and the field takes that type (`Option<…>` when the relation
    /// is optional). A to-many nest (`items { … }`) becomes a nested struct wrapped in
    /// `Vec<…>`. A `field -> Shape` nest references the named shape's own struct
    /// instead of minting a per-parent one, so every site shares one nominal type;
    /// `stack` holds the shape names mid-expansion (a cycle guard — sema rejects
    /// reference cycles, this keeps the emitter terminating regardless).
    fn build_struct(
        schema: &CheckedSchema,
        decls: &[Decl],
        name: String,
        body: &[ShapeField],
        model: Option<&RModel>,
        stack: &mut Vec<String>,
    ) -> OutStruct {
        let mut fields = Vec::new();
        let mut nested = Vec::new();
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
                ShapeField::Nest { field, body } => {
                    if let Some((target, optional)) = to_one_relation(schema, model, &field.node) {
                        let sub_name = format!("{name}{}", pascal(&field.node));
                        let sub = build_struct(
                            schema,
                            decls,
                            sub_name.clone(),
                            body,
                            Some(target),
                            stack,
                        );
                        let ty = if optional {
                            format!("Option<{sub_name}>")
                        } else {
                            sub_name
                        };
                        fields.push((field.node.clone(), ty));
                        nested.push(sub);
                    } else if let Some(target) = to_many_relation(schema, model, &field.node) {
                        // A to-many nest is a JSON array of the element struct: `Vec<Sub>`.
                        let sub_name = format!("{name}{}", pascal(&field.node));
                        let sub = build_struct(
                            schema,
                            decls,
                            sub_name.clone(),
                            body,
                            Some(target),
                            stack,
                        );
                        fields.push((field.node.clone(), format!("Vec<{sub_name}>")));
                        nested.push(sub);
                    }
                }
                ShapeField::NestRef { field, shape } => {
                    let Some(decl) = find_shape(decls, &shape.node) else {
                        continue;
                    };
                    if !stack.contains(&shape.node) {
                        // Build the referenced shape's own struct (emitted once,
                        // deduped by name across every referencing site).
                        stack.push(shape.node.clone());
                        let sub = build_struct(
                            schema,
                            decls,
                            shape.node.clone(),
                            &decl.body,
                            schema.model(&decl.from.node),
                            stack,
                        );
                        stack.pop();
                        nested.push(sub);
                    }
                    if let Some((_, optional)) = to_one_relation(schema, model, &field.node) {
                        let ty = if optional {
                            format!("Option<{}>", shape.node)
                        } else {
                            shape.node.clone()
                        };
                        fields.push((field.node.clone(), ty));
                    } else if to_many_relation(schema, model, &field.node).is_some() {
                        fields.push((field.node.clone(), format!("Vec<{}>", shape.node)));
                    }
                }
            }
        }
        OutStruct {
            name,
            fields,
            nested,
        }
    }

    /// The target model + `optional` of a **to-one** relation field, or `None` for a
    /// scalar, an unknown field, or a to-**many** edge (a Forward is always to-one; an
    /// Inverse is to-one only when its paired forward FK is unique — a one-to-one back
    /// edge, which may be absent, hence optional). Mirrors the SQL side's `enter_to_one`.
    fn to_one_relation<'a>(
        schema: &'a CheckedSchema,
        model: Option<&RModel>,
        field: &str,
    ) -> Option<(&'a RModel, bool)> {
        match model?.member(field).map(|m| &m.kind)? {
            MemberKind::Forward {
                target, optional, ..
            } => schema.model(target).map(|t| (t, *optional)),
            MemberKind::Inverse { target, via } => {
                let t = schema.model(target)?;
                t.is_unique(via).then_some((t, true))
            }
            MemberKind::Scalar { .. } => None,
        }
    }

    /// The target model of a to-**many** relation field (an Inverse collection — its
    /// paired forward FK is *not* unique), or `None` for a scalar / to-one edge. Mirrors
    /// the SQL side's `to_many_edge`; the client renders it `Vec<Sub>`.
    fn to_many_relation<'a>(
        schema: &'a CheckedSchema,
        model: Option<&RModel>,
        field: &str,
    ) -> Option<&'a RModel> {
        match model?.member(field).map(|m| &m.kind)? {
            MemberKind::Inverse { target, via } => {
                let t = schema.model(target)?;
                (!t.is_unique(via)).then_some(t)
            }
            _ => None,
        }
    }

    /// Emit an output struct and its to-one nested aux structs, deduped by name across
    /// callables (a shape shared by two queries is one struct).
    fn emit_struct(out: &mut String, s: &OutStruct, seen: &mut Vec<String>) {
        if seen.contains(&s.name) {
            return;
        }
        seen.push(s.name.clone());
        out.push('\n');
        out.push_str(&render_struct(&s.name, &s.fields));
        for n in &s.nested {
            emit_struct(out, n, seen);
        }
    }

    /// Every stored column of a bare-model return: scalars by their type (the `id`
    /// column as this model's typed id), forward FKs as the target's typed id under the
    /// relation field name (matching the SELECT alias). Inverse edges store nothing, so
    /// they are omitted.
    fn model_fields(model: &RModel) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        for mem in &model.members {
            match &mem.kind {
                MemberKind::Scalar {
                    ty: Primitive::Id,
                    optional,
                    many,
                    ..
                } => fields.push((
                    mem.name.clone(),
                    wrap(&id_type(&model.name), *optional, *many),
                )),
                MemberKind::Scalar {
                    ty, optional, many, ..
                } => fields.push((mem.name.clone(), wrap(primitive(*ty), *optional, *many))),
                MemberKind::Forward {
                    target, optional, ..
                } => fields.push((mem.name.clone(), wrap(&id_type(target), *optional, false))),
                MemberKind::Inverse { .. } => {}
            }
        }
        fields
    }

    // ---------- input structs ----------------------------------------------

    /// The input fields for a callable: one per signature param, typed from its
    /// explicit annotation or inferred from the column it maps to.
    fn input_fields(schema: &CheckedSchema, c: &Callable) -> Vec<(String, String)> {
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
    fn page_input(q: &Query) -> PageInput {
        let clauses: &[Clause] = match &q.body {
            QueryBody::Inline(cs) => cs,
            QueryBody::Block(s) => &s.clauses,
            QueryBody::Bare => return PageInput::None,
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
        let optional = p.default.is_some() || p.ty.as_ref().is_some_and(|t| t.optional);
        let base = if let Some(entity) = param_entity(c, p) {
            let many = p.ty.as_ref().is_some_and(|t| t.many);
            wrap(&id_type(&entity), false, many)
        } else {
            match &p.ty {
                Some(te) => wrap(base_type(&te.base), false, te.many),
                None => infer_param(schema, c.root, p),
            }
        };
        if optional && !base.starts_with("Option<") {
            format!("Option<{base}>")
        } else {
            base
        }
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
                // The model's own `id` is that model's typed id.
                Some(MemberKind::Scalar {
                    ty: Primitive::Id,
                    optional,
                    many,
                    ..
                }) => return wrap(&id_type(&cur.name), *optional, *many),
                Some(MemberKind::Scalar {
                    ty, optional, many, ..
                }) => return wrap(primitive(*ty), *optional, *many),
                Some(MemberKind::Forward {
                    target, optional, ..
                }) => {
                    if last {
                        return wrap(&id_type(target), *optional, false);
                    }
                    match schema.model(target) {
                        Some(m) => cur = m,
                        None => return "Json".to_string(),
                    }
                }
                Some(MemberKind::Inverse { target, .. }) => {
                    if last {
                        // Terminal to-many reach: a collection of the target's typed ids.
                        return format!("Vec<{}>", id_type(target));
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
    /// `Uuid` FK the wire carries.
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

    /// One typed client method: `POST` the input to the route, carry the typed
    /// context, decode the output. A callable with `$ctx` requirements takes a
    /// `<Name>Ctx`; one with none takes `ctx: ()` (the engine reads no context).
    fn render_method(c: &Callable) -> String {
        let ctx_ty = if c.ctx_requires.is_empty() {
            "()".to_string()
        } else {
            ctx_name(c.name)
        };
        format!(
            "    /// `POST {route}`\n    pub fn {name}(&self, input: {input}, ctx: {ctx_ty}) -> Result<{output}, ClientError> {{\n        self.transport.call({konst}, &input, &ctx)\n    }}\n",
            route = c.route,
            name = field_ident(c.name),
            input = input_name(c.name),
            ctx_ty = ctx_ty,
            output = c.output,
            konst = route_const(c.name),
        )
    }

    /// The context fields for a callable: one per required `$ctx.<field>`, typed by
    /// the inference (a relation requirement carries the model's key `Uuid`).
    fn ctx_fields(reqs: &[CtxReq]) -> Vec<(String, String)> {
        reqs.iter()
            .map(|r| (r.field.clone(), ctx_field_type(&r.ty)))
            .collect()
    }

    /// A `$ctx` field's Rust type: a scalar by its alias, a relation as that model's
    /// typed id (`Id<entity::M>`) — the same mapping the input side uses.
    fn ctx_field_type(ty: &CtxField) -> String {
        match ty {
            CtxField::Scalar(p) => primitive(*p).to_string(),
            CtxField::Relation(model) => id_type(model),
        }
    }

    fn input_name(name: &str) -> String {
        format!("{}Input", pascal(name))
    }

    fn ctx_name(name: &str) -> String {
        format!("{}Ctx", pascal(name))
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
// The closed RPC surface: one input type, one output type, and one route per
// signature. Transport is abstract — implement `Transport` to post JSON to a route
// and decode the reply; the runtime supplies the concrete HTTP client.
//
// Some generated items may be unused by a given consumer; suppress dead-code warnings by
// including this module under an outer `#[allow(dead_code)] mod client { … }` (an inner
// `#![allow]` would be rejected by `include!`).

use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

// Semantic aliases for the wire types (mirrors the DDL mapping).
pub type Uuid = String;
pub type Timestamp = String;
pub type Date = String;
pub type Json = serde_json::Value;

/// A typed id: the primary key of entity `E`, carried on the wire as its raw string
/// (`#[serde(transparent)]`, so the wire is unchanged). The `E` marker keeps ids of
/// different entities distinct types, so a `User` id can't be passed where an `Org` id
/// is wanted. A `create_*` result already hands one back typed; turn a raw string into
/// one only through the explicit, greppable `Id::from_raw`.
#[derive(Serialize, Deserialize)]
#[serde(transparent, bound = "")]
pub struct Id<E> {
    raw: String,
    #[serde(skip)]
    _entity: PhantomData<fn() -> E>,
}

impl<E> Id<E> {
    /// Wrap a raw id string as a typed id — the explicit escape from an untyped string,
    /// used only where the string's entity is known (an id from outside the client).
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Id {
            raw: raw.into(),
            _entity: PhantomData,
        }
    }
    /// The underlying id string.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
    /// Consume into the raw id string.
    pub fn into_raw(self) -> String {
        self.raw
    }
}

// Hand-written so the marker `E` carries no trait bounds (a derive would demand
// `E: Clone`, `E: Ord`, … of a type that only ever tags).
impl<E> Clone for Id<E> {
    fn clone(&self) -> Self {
        Id {
            raw: self.raw.clone(),
            _entity: PhantomData,
        }
    }
}
impl<E> std::fmt::Debug for Id<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Id({:?})", self.raw)
    }
}
impl<E> std::fmt::Display for Id<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}
impl<E> PartialEq for Id<E> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<E> Eq for Id<E> {}
impl<E> PartialOrd for Id<E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<E> Ord for Id<E> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}
impl<E> std::hash::Hash for Id<E> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

/// An opaque keyset pagination cursor, carried on the wire as its underlying string
/// (`#[serde(transparent)]`, so the wire is unchanged). A page result hands one back and
/// the caller feeds it to the next call; its contents (the sort-key basis) are a runtime
/// concern the caller never assembles. Turn a raw string into one only through the
/// explicit, greppable `Cursor::from_raw`.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(String);

impl Cursor {
    /// Wrap a raw cursor string — the explicit escape used only where a cursor string
    /// arrives from outside the client (normally a page result already hands one back typed).
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Cursor(raw.into())
    }
    /// The underlying cursor string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Consume into the raw cursor string.
    pub fn into_raw(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cursor({:?})", self.0)
    }
}
impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Pagination envelope: a paginated query returns rows + an opaque cursor.
/// Next page = the same call carrying `cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub cursor: Option<Cursor>,
}

/// What went wrong in a client call — lets a caller branch on the class of failure
/// without matching on the message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientErrorKind {
    /// The request never completed a round-trip — a socket/connection/engine failure.
    /// Retryable in the same way the underlying transport is.
    Transport,
    /// The round-trip completed but a value could not be (de)serialized into its typed
    /// input or output. A bug in the caller's types or the payload, not a server error.
    Decode,
    /// The server ran the call and returned a structured error (its HTTP `status` and a
    /// stable machine `code`, e.g. `bad_arg`, `missing_ctx`, `database_error`).
    Api { status: u16, code: String },
}

/// An error from a client call: a transport failure, a (de)serialization failure, or a
/// structured error the server returned. Carries a stable machine [`code`](ClientError::code)
/// and a clear human message; a server error also carries its HTTP [`status`](ClientError::status).
/// Implements `std::error::Error`, so it chains with `?` and its underlying cause is reachable
/// via `source()`.
#[derive(Debug, Clone)]
pub struct ClientError {
    kind: ClientErrorKind,
    message: String,
    // Kept behind an `Arc` so `ClientError` stays `Clone` while still handing back a live
    // `&dyn Error` from `source()`.
    source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync + 'static>>,
}

impl ClientError {
    /// A transport failure (the round-trip never completed), wrapping its cause.
    pub fn transport(err: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        let err = err.into();
        ClientError {
            kind: ClientErrorKind::Transport,
            message: err.to_string(),
            source: Some(err.into()),
        }
    }

    /// A (de)serialization failure decoding an input or the reply, wrapping its cause.
    pub fn decode(err: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        let err = err.into();
        ClientError {
            kind: ClientErrorKind::Decode,
            message: err.to_string(),
            source: Some(err.into()),
        }
    }

    /// A structured error the server returned: its HTTP `status`, stable machine `code`, and
    /// human `message` (the `{ error: { code, message } }` envelope).
    pub fn api(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        ClientError {
            kind: ClientErrorKind::Api {
                status,
                code: code.into(),
            },
            message: message.into(),
            source: None,
        }
    }

    /// The failure class (transport / decode / server-side api).
    pub fn kind(&self) -> &ClientErrorKind {
        &self.kind
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A stable machine-readable code: the server's `error.code` for an api failure, else
    /// `"transport"` / `"decode"`. Safe to branch on.
    pub fn code(&self) -> &str {
        match &self.kind {
            ClientErrorKind::Transport => "transport",
            ClientErrorKind::Decode => "decode",
            ClientErrorKind::Api { code, .. } => code,
        }
    }

    /// The HTTP status of a server-side (api) failure; `None` for a transport/decode failure.
    pub fn status(&self) -> Option<u16> {
        match &self.kind {
            ClientErrorKind::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ClientErrorKind::Transport => write!(f, "transport error: {}", self.message),
            ClientErrorKind::Decode => write!(f, "decode error: {}", self.message),
            ClientErrorKind::Api { status, code } => {
                write!(f, "server error {status} [{code}]: {}", self.message)
            }
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| &**e as &(dyn std::error::Error + 'static))
    }
}

/// Post a typed input to a route, carry the typed request context (`$ctx`, carried out
/// of band as request context), and decode the typed output. A callable with no `$ctx`
/// requirements passes `ctx: &()`. Implemented by the runtime's HTTP client; codegen
/// only depends on this shape.
pub trait Transport {
    fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
    where
        I: Serialize,
        C: Serialize,
        O: serde::de::DeserializeOwned;
}

/// The generated client, generic over a `Transport`.
pub struct Client<T> {
    pub transport: T,
}
"#;

    /// The opt-in in-process bridge, appended when [`ClientOptions::embedded`] is set. It
    /// references `based_runtime::Engine` *by path* — the consuming crate depends on
    /// based-runtime (based-codegen itself does not; that would be circular). This is the
    /// bridge an embedder would otherwise hand-copy: serialize the typed input and `$ctx`
    /// to JSON (a non-object ctx → `{}`), call `engine.call`, decode a `200` body into
    /// `O`, map a non-`200` to a `ClientError` from `error.message`.
    const EMBEDDED: &str = r#"
// ---------- embedded bridge (based_runtime::Engine) ----------

/// A `Transport` backed by an in-process `based_runtime::Engine` — every callable runs
/// through the engine's dispatch core with no socket. Build one with [`embedded`].
pub struct Embedded<'a> {
    engine: &'a based_runtime::Engine,
}

impl Transport for Embedded<'_> {
    fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
    where
        I: Serialize,
        C: Serialize,
        O: serde::de::DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(ClientError::decode)?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { serde_json::json!({}) })
            .map_err(ClientError::decode)?;
        let resp = self.engine.call(route, args, ctx);
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(ClientError::decode)
        } else {
            // Preserve the server's structured error: its status + stable code + message.
            let code = resp.body["error"]["code"].as_str().unwrap_or("error");
            let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
            Err(ClientError::api(resp.status, code, message))
        }
    }
}

/// A ready-to-use client over an in-process `based_runtime::Engine` — no bridge to write.
/// `$ctx` is a typed per-call argument the app sets, not the caller; a public callable
/// passes `()`, which maps to an empty context bag.
pub fn embedded(engine: &based_runtime::Engine) -> Client<Embedded<'_>> {
    Client {
        transport: Embedded { engine },
    }
}
"#;
}
