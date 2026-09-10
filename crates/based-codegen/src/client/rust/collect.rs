use super::*;

/// Build the callable descriptors from the checked schema + AST.
pub(super) fn collect<'a>(schema: &'a CheckedSchema, decls: &'a [Decl]) -> Vec<Callable<'a>> {
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
        let callable = match decl {
            Decl::Query(q) => collect_query(schema, decls, q, &queries),
            Decl::Mutation(m) => collect_mutation(schema, decls, m, &mutations),
            _ => None,
        };
        if let Some(c) = callable {
            out.push(c);
        }
    }
    out
}

/// Lower one query decl to its callable descriptor.
fn collect_query<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    q: &'a Query,
    queries: &std::collections::HashMap<&str, &'a RQuery>,
) -> Option<Callable<'a>> {
    let rq = queries.get(q.name.node.as_str())?;
    let root = schema.model(&rq.target);
    let os = out_struct(schema, decls, &q.ret, root);
    Some(Callable {
        name: &q.name.node,
        route: format!("/q/{}", q.name.node),
        params: &q.params,
        root,
        output: query_output(rq, &os.name),
        stream: rq.stream,
        is_mutation: false,
        ack: false,
        out_struct: os,
        ctx_requires: &rq.ctx_requires,
        page: page_input(q),
        // A raw body voids the same-name column convention: its params
        // are pure bind values, typed by their (mandatory) annotations.
        param_entities: if matches!(q.body, QueryBody::Raw(_)) {
            std::collections::HashMap::new()
        } else {
            based_sema::query_param_entities(schema, root, &q.params)
        },
        for_update: matches!(&q.body, QueryBody::Block(s) if s.for_update.is_some()),
    })
}

/// Lower one mutation decl to its callable descriptor. An `-> ok` mutation names no
/// shape/model, so the primary written model types the params and the output is unit.
fn collect_mutation<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    m: &'a Mutation,
    mutations: &std::collections::HashMap<&str, &'a based_sema::RMutation>,
) -> Option<Callable<'a>> {
    let rm = mutations.get(m.name.node.as_str())?;
    let root = if rm.ack {
        schema.model(&rm.ret_model)
    } else {
        schema.model(&m.ret.ty.node).or_else(|| {
            // A shape return: resolve the model it projects from.
            schema
                .shapes
                .iter()
                .find(|s| s.name == m.ret.ty.node)
                .and_then(|s| schema.model(&s.from))
        })
    };
    let (os, output) = if rm.ack {
        (
            OutStruct {
                name: String::new(),
                fields: Vec::new(),
                nested: Vec::new(),
            },
            "()".to_string(),
        )
    } else {
        let os = out_struct(schema, decls, &m.ret, root);
        let output = if m.ret.many {
            format!("Vec<{}>", os.name)
        } else {
            os.name.clone()
        };
        (os, output)
    };
    Some(Callable {
        name: &m.name.node,
        route: format!("/m/{}", m.name.node),
        params: &m.params,
        root,
        output,
        stream: false,
        is_mutation: true,
        ack: rm.ack,
        out_struct: os,
        ctx_requires: &rm.ctx_requires,
        page: PageInput::None,
        param_entities: based_sema::mutation_param_entities(schema, m),
        for_update: false,
    })
}
