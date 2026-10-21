use super::*;
use crate::shape_filters::shape_filter_uses;

/// A query or mutation that returns a shape, as its filter projections' `$p` binder.
pub(crate) struct ShapeBinder<'a> {
    pub(crate) callable: &'a Ident,
    pub(crate) params: &'a [Param],
    pub(crate) ret_span: Span,
    /// Untyped params map onto same-named columns (a bare/inline query).
    pub(crate) infer: bool,
}

/// Every `$p` the returned shape's filter projections read must be a param of the
/// callable whose type agrees with the filter's use of it (`E0355`).
pub(crate) fn check_shape_params(
    b: &ShapeBinder,
    shape: &str,
    ti: usize,
    cx: &Cx,
    sink: &mut Sink,
) {
    let Some(body) = cx.shape_bodies.get(shape).copied() else {
        return;
    };
    for u in shape_filter_uses(body, ti, cx) {
        let Some(def) = cx.filters.get(&u.call.name.node).copied() else {
            continue;
        };
        for (arg, fparam) in u.call.args.iter().zip(&def.params) {
            let Value::Param(pr) = arg else { continue };
            if pr.name.node == "ctx" {
                continue;
            }
            let reads = format!(
                "`{shape}.{}` reads `${}` through filter `{}`",
                u.out.node, pr.name.node, def.name.node
            );
            let Some(p) = b.params.iter().find(|p| p.name.node == pr.name.node) else {
                sink.error_note(
                    code::SHAPE_PARAM_UNBOUND,
                    b.ret_span,
                    format!(
                        "`{}` has no param `{}` for its shape",
                        b.callable.node, pr.name.node
                    ),
                    reads,
                );
                continue;
            };
            let col = resolve::filter_param_column(def, &fparam.name.node, u.model, cx);
            let have = param_family(p, ti, b.infer, cx);
            if let (Some(col), Some(have)) = (col, have) {
                if !resolve::compatible(resolve::terminal_family(&col), have) {
                    sink.error_note(
                        code::SHAPE_PARAM_UNBOUND,
                        p.name.span,
                        format!(
                            "param `{}` is {} but its shape's filter compares it to {}",
                            p.name.node,
                            resolve::family_name(have),
                            resolve::terminal_name(&col)
                        ),
                        reads,
                    );
                }
            }
        }
    }
}
