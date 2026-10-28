use super::*;

/// Every param a `~ all` / `~ any` matches against — written directly, bound by a signature
/// `~ all col`, or passed through a named filter's argument — must be declared `text[]`
/// (E0152): the client sends it as a list and the runtime expands it into one bind per pattern.
pub(crate) fn check_like_list_params<'a>(
    params: &[Param],
    preds: impl IntoIterator<Item = &'a Predicate>,
    cx: &Cx,
    sink: &mut Sink,
) {
    for p in params {
        if let Some(ParamBinding::ColOp { op, .. }) = &p.binding {
            if is_like_list(*op) {
                require_text_list(p, &p.name, *op, sink);
            }
        }
    }
    let mut uses = Vec::new();
    for pred in preds {
        like_list_uses(pred, &Scope::Callable, cx, &mut Vec::new(), &mut uses);
    }
    for (op, pr) in uses {
        if let Some(p) = params.iter().find(|p| p.name.node == pr.name.node) {
            require_text_list(p, &pr.name, op, sink);
        }
    }
}

/// The `where` / `having` predicates of a query body.
pub(crate) fn query_predicates(q: &Query) -> impl Iterator<Item = &Predicate> {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    clauses.iter().filter_map(|c| match c {
        Clause::Where(p) | Clause::Having(p) => Some(p),
        _ => None,
    })
}

/// The `where` predicates of a mutation's writes, through `tx` blocks.
pub(crate) fn write_predicates(stmts: &[WriteStmt]) -> Vec<&Predicate> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            WriteStmt::Update { where_, .. } | WriteStmt::Restore { where_, .. } => {
                out.push(where_);
            }
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                out.extend(where_.as_ref());
            }
            WriteStmt::Tx(inner) => out.extend(write_predicates(inner)),
            WriteStmt::Create { .. } | WriteStmt::Raw(_) => {}
        }
    }
    out
}

fn is_like_list(op: Op) -> bool {
    matches!(op, Op::LikeAll | Op::LikeAny)
}

fn require_text_list(p: &Param, at: &Ident, op: Op, sink: &mut Sink) {
    let is_text_list =
        p.ty.as_ref()
            .is_some_and(|t| t.many && matches!(t.base, BaseType::Primitive(Primitive::Text)));
    if !is_text_list {
        sink.error_note(
            code::PARAM_TYPE,
            at.span,
            format!(
                "`{}` matches a pattern list, but param `{}` is not `text[]`",
                resolve::op_sym(op),
                p.name.node
            ),
            format!("declare it `{}: text[]`", p.name.node),
        );
    }
}

/// How a `$name` in the predicate being walked reaches the callable: directly, or through
/// the arguments of the named-filter call whose body this is.
enum Scope<'a, 'p> {
    Callable,
    Filter {
        params: &'a [Param],
        args: &'a [Value],
        outer: &'p Self,
    },
}

impl<'a> Scope<'a, '_> {
    /// The callable param a `$name` resolves to, if it resolves to one.
    fn callable_param(&self, pr: &'a ParamRef) -> Option<&'a ParamRef> {
        match self {
            Scope::Callable => Some(pr),
            Scope::Filter {
                params,
                args,
                outer,
            } => {
                let i = params.iter().position(|p| p.name.node == pr.name.node)?;
                match args.get(i)? {
                    Value::Param(arg) => outer.callable_param(arg),
                    _ => None,
                }
            }
        }
    }
}

fn like_list_uses<'a>(
    pred: &'a Predicate,
    scope: &Scope<'a, '_>,
    cx: &Cx<'a>,
    in_filters: &mut Vec<&'a str>,
    out: &mut Vec<(Op, &'a ParamRef)>,
) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            like_list_uses(a, scope, cx, in_filters, out);
            like_list_uses(b, scope, cx, in_filters, out);
        }
        Predicate::Not(p) => like_list_uses(p, scope, cx, in_filters, out),
        Predicate::Any { pred: Some(p), .. } => like_list_uses(p, scope, cx, in_filters, out),
        Predicate::Cmp {
            op,
            value: Value::Param(pr),
            ..
        } if is_like_list(*op) => {
            if let Some(param) = scope.callable_param(pr) {
                out.push((*op, param));
            }
        }
        Predicate::FilterCall { name, args } => {
            let Some(def) = cx.filters.get(&name.node).copied() else {
                return;
            };
            if in_filters.contains(&def.name.node.as_str()) {
                return;
            }
            in_filters.push(def.name.node.as_str());
            let inner = Scope::Filter {
                params: &def.params,
                args,
                outer: scope,
            };
            like_list_uses(&def.pred, &inner, cx, in_filters, out);
            in_filters.pop();
        }
        _ => {}
    }
}
