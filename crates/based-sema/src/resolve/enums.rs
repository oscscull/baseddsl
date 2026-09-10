use super::*;

/// Check a value written against an enum-typed column (`where status = paid`,
/// `create { status: paid }`). A bare single-segment identifier is a variant — checked
/// for membership. A `$param` is still name-checked. Returns `true` when it
/// fully handled the value (so the caller skips the ordinary column-path resolution that
/// would misread the variant as a field); `false` to fall through (a string literal, a
/// null, etc., which the ordinary text-family check then covers).
pub fn check_enum_operand(value: &Value, en: &REnum, params: &[String], sink: &mut Sink) -> bool {
    match value {
        Value::Path(p) if p.segments.len() == 1 => {
            let seg = &p.segments[0];
            if !en.has_variant(&seg.node) {
                sink.error(
                    code::ENUM_VARIANT,
                    seg.span,
                    format!(
                        "`{}` is not a variant of enum `{}` (expected one of: {})",
                        seg.node,
                        en.name,
                        en.variant_names().join(", ")
                    ),
                );
            }
            true
        }
        Value::Param(pr) => {
            check_param_ref(pr, params, sink);
            true
        }
        _ => false,
    }
}

/// Why a column is ineligible for an aggregate, or `None` when it is fine.
/// `sum`/`avg` need the numeric family; `min`/`max` need a *comparable* column (numeric,
/// `timestamp`, `date`, `text`); `count` is arg-less so never reaches here. An enum or a
/// relation is never a numeric/comparable aggregate operand.
pub fn agg_operand_reason(func: &str, term: &Terminal, is_enum: bool) -> Option<String> {
    let numeric = matches!(
        term,
        Terminal::Scalar(Primitive::Int | Primitive::Float | Primitive::Decimal { .. })
    ) && !is_enum;
    let comparable = numeric
        || matches!(
            term,
            Terminal::Scalar(
                Primitive::Timestamp | Primitive::Date | Primitive::Time | Primitive::Text
            )
        ) && !is_enum;
    match func {
        "sum" | "avg" if !numeric => Some(format!(
            "`{func}` needs a numeric column (int/float/decimal), not {}",
            terminal_name(term)
        )),
        "min" | "max" if !comparable => Some(format!(
            "`{func}` needs a comparable column (numeric/timestamp/date/time/text), not {}",
            terminal_name(term)
        )),
        _ => None,
    }
}
