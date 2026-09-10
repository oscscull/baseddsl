use super::*;


/// Every model/type-reference identifier across the AST, with its span — the sites
/// a name *points at* a declared model or shape (not the declarations themselves).
/// Traverses all reference-bearing positions: field types, opt-in inverses, shape
/// `from`, query/mutation return types + param types + `get`/`list` targets, write
/// targets (incl. nested `tx`), and filter param types.
pub(crate) fn collect_type_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Model(m) => {
                for member in &m.members {
                    if let Member::Field(f) = member {
                        collect_type_expr(&f.ty, &mut out);
                        if let Some(inv) = &f.inverse {
                            out.push(&inv.model);
                        }
                    }
                }
            }
            Decl::Shape(s) => {
                out.push(&s.from);
                collect_shape_body_refs(&s.body, &mut out);
            }
            Decl::Scope(s) => {
                for t in &s.terms {
                    collect_type_expr(&t.ty, &mut out);
                }
            }
            Decl::Query(q) => {
                if !q.ret.ack {
                    out.push(&q.ret.ty);
                }
                for p in &q.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
                if let QueryBody::Block(stmt) = &q.body {
                    out.push(&stmt.model);
                }
            }
            Decl::Mutation(m) => {
                // `-> ok` is the ack token, not a type reference.
                if !m.ret.ack {
                    out.push(&m.ret.ty);
                }
                for p in &m.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
                collect_write_targets(&m.body, &mut out);
            }
            Decl::Filter(f) => {
                for p in &f.params {
                    if let Some(ty) = &p.ty {
                        collect_type_expr(ty, &mut out);
                    }
                }
            }
            // An enum decl has no outgoing type references (its variants are its own).
            Decl::Enum(_) => {}
        }
    }
    out
}


/// Collect every navigable column path in a computed shape field (`out = price - discount`
/// / `a || b` / `case …`), rooted at the shape's model `from` — its arithmetic/concat
/// operands and its CASE `when` comparison columns, so each is a go-to-def / rename site.
pub(crate) fn computed_paths<'a>(expr: &'a ShapeExpr, from: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match expr {
        ShapeExpr::Value(Value::Path(p)) => out.push((from, &p.segments)),
        ShapeExpr::Value(_) => {}
        ShapeExpr::Arith { lhs, rhs, .. } | ShapeExpr::Concat { lhs, rhs, .. } => {
            computed_paths(lhs, from, out);
            computed_paths(rhs, from, out);
        }
        ShapeExpr::Case { arms, else_, .. } => {
            for arm in arms {
                pred_column_paths(&arm.when, from, out);
                computed_paths(&arm.then, from, out);
            }
            computed_paths(else_, from, out);
        }
    }
}


/// The column paths a predicate compares on (a CASE `when` condition), rooted at `from`.
pub(crate) fn pred_column_paths<'a>(p: &'a Predicate, from: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            pred_column_paths(a, from, out);
            pred_column_paths(b, from, out);
        }
        Predicate::Not(inner) => pred_column_paths(inner, from, out),
        Predicate::Cmp { path, .. } | Predicate::InList { path, .. } | Predicate::Bare(path) => {
            out.push((from, &path.segments));
        }
        Predicate::FilterCall { .. } | Predicate::Raw(_) => {}
    }
}


/// The `field -> Shape` references and `...Shape` spreads in a shape body (recursing
/// through inline nests) — each names a shape decl, so it rides the type-reference index
/// (go-to-def, find-references, rename).
pub(crate) fn collect_shape_body_refs<'a>(body: &'a [ShapeField], out: &mut Vec<&'a Ident>) {
    for f in body {
        match f {
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                collect_shape_body_refs(body, out);
            }
            ShapeField::NestRef { shape, .. } | ShapeField::Spread { shape } => out.push(shape),
            ShapeField::Bare(_) | ShapeField::Rename { .. } => {}
        }
    }
}


/// The model reference in a type expression, if its base is a model (not a primitive).
pub(crate) fn collect_type_expr<'a>(ty: &'a TypeExpr, out: &mut Vec<&'a Ident>) {
    if let BaseType::Model(id) = &ty.base {
        out.push(id);
    }
}
