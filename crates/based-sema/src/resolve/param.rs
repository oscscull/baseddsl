use super::*;

/// The column a param maps onto, for annotation agreement .
pub enum Mapped<'a> {
    Scalar(Primitive),
    Relation(&'a str),
}

/// An explicit param annotation must agree with the column it maps onto. A
/// relation param may be typed as its target model *or* as its key (`Id`/`Uuid`);
/// a scalar param must match the column's family. Loose on purpose (family, not
/// exact primitive) so `Uuid`↔`Id` and the like don't spuriously conflict.
pub fn check_param_type(
    ann: &TypeExpr,
    mapped: Mapped,
    target_key: Option<Primitive>,
    sink: &mut Sink,
) {
    match (&ann.base, mapped) {
        (BaseType::Primitive(pann), Mapped::Scalar(pcol)) => {
            if prim_family(*pann) != prim_family(pcol) {
                sink.error(
                    code::PARAM_TYPE,
                    ann.span,
                    format!(
                        "param typed `{}` maps to a `{}` column",
                        prim_name(*pann),
                        prim_name(pcol)
                    ),
                );
            }
        }
        (BaseType::Primitive(pann), Mapped::Relation(target)) => {
            if !matches!(pann, Primitive::Id | Primitive::Uuid) {
                sink.error(
                    code::PARAM_TYPE,
                    ann.span,
                    format!(
                        "param typed `{}` binds relation `{target}`; use the model type or a key (`Id`)",
                        prim_name(*pann)
                    ),
                );
            } else if let Some(key) = target_key {
                // `Id` resolves to whatever the target's key is; an explicit concrete key
                // type must match it — a `serial` (integer-keyed) target rejects a `uuid`
                // param (the runtime would coerce it as a string and reject the number).
                if *pann != Primitive::Id && prim_family(*pann) != prim_family(key) {
                    sink.error(
                        code::PARAM_TYPE,
                        ann.span,
                        format!(
                            "param typed `{}` binds relation `{target}`, whose key is `{}` — annotate `Id` or `{}`",
                            prim_name(*pann),
                            prim_name(key),
                            prim_name(key)
                        ),
                    );
                }
            }
        }
        (BaseType::Model(m), Mapped::Relation(target)) => {
            if m.node != target {
                sink.error(
                    code::PARAM_TYPE,
                    m.span,
                    format!("param typed `{}` binds a relation to `{target}`", m.node),
                );
            }
        }
        // The parser rejects `raw(…)` outside a model field type, so an opaque
        // annotation never reaches here.
        (BaseType::Raw(_), _) => {}
        (BaseType::Model(m), Mapped::Scalar(pcol)) => sink.error(
            code::PARAM_TYPE,
            m.span,
            format!(
                "param typed `{}` (a model) maps to a `{}` column",
                m.node,
                prim_name(pcol)
            ),
        ),
    }
}

pub fn check_param_ref(pr: &ParamRef, params: &[String], sink: &mut Sink) {
    // `$ctx` is the caller-supplied request context . It must be referenced
    // as exactly `$ctx.<field>` (one segment — the fields are flat). Its *type* is
    // not declared: it is inferred per callable from the column each use compares
    // against, and checked for cross-callable coherence (see `ctx.rs`).
    if pr.name.node == "ctx" {
        if pr.path.len() != 1 {
            let span = pr.path.last().map_or(pr.name.span, |s| s.span);
            sink.error(
                code::CTX_BAD_PATH,
                span,
                "`$ctx` takes exactly one field (e.g. `$ctx.org`)",
            );
        }
        return;
    }
    if !params.iter().any(|p| p == &pr.name.node) {
        sink.error(
            code::UNKNOWN_PARAM,
            pr.name.span,
            format!("unknown parameter `${}`", pr.name.node),
        );
    }
}
