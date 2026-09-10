use super::*;

/// Resolve the structural facts that only touch *this* model plus name lookups:
/// relation targets, inverse pairings, decorator roles, indexes, and uniqueness.
/// Expression resolution (scope/sort paths, which traverse *other* models) is done
/// afterwards in [`resolve_exprs`], once every model is fully built — otherwise the
/// read-only path checker would alias this `&mut` pass.
pub fn validate(
    ast: &Model,
    mi: usize,
    models: &mut [RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    validate_relations(mi, models, index, sink);
    validate_indexes(ast, mi, models, sink);
    validate_decorators(ast, mi, models, sink);
    validate_fk(ast, mi, models, sink);
    validate_was(ast, mi, models, sink);
    validate_decimals(ast, sink);
    validate_time_bytes(ast, sink);
    compute_unique(ast, &mut models[mi]);
}

/// The largest `decimal` precision the engine's type map guarantees across dialects.
const DECIMAL_MAX_PRECISION: u32 = 38;

/// Check every `decimal(p, s)` field's precision/scale is in range (`1 ≤ s ≤ p ≤ 38`)
/// and that a decimal column's `default` is a decimal literal (an integer or a fractional
/// literal), not a string/bool — both. Purely local (one model's own fields).
pub(crate) fn validate_decimals(ast: &Model, sink: &mut Sink) {
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let BaseType::Primitive(Primitive::Decimal { precision, scale }) = f.ty.base else {
            continue;
        };
        if !(1 <= scale && scale <= precision && precision <= DECIMAL_MAX_PRECISION) {
            sink.error(
                code::DECIMAL_INVALID,
                f.ty.span,
                format!(
                    "`decimal({precision}, {scale})` is out of range — need \
                     1 ≤ scale ≤ precision ≤ {DECIMAL_MAX_PRECISION}"
                ),
            );
        }
        for m in &f.modifiers {
            let Modifier::Default(DefaultVal::Lit(lit)) = m else {
                continue;
            };
            if !matches!(lit, Literal::Int(_) | Literal::Decimal(_) | Literal::Null) {
                sink.error(
                    code::DECIMAL_INVALID,
                    f.span,
                    format!(
                        "default for decimal column `{}` must be a decimal literal",
                        f.name.node
                    ),
                );
            }
        }
    }
}

/// Validate `time`/`bytes` column defaults. A `time` default must be a string literal
/// (a time-of-day like `"14:30:00"`) — a non-string is. A `bytes` column cannot
/// carry any literal default (there is no source spelling for a binary blob) —;
/// supply it from a raw migration or a DB default instead. Purely local.
pub(crate) fn validate_time_bytes(ast: &Model, sink: &mut Sink) {
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let BaseType::Primitive(prim) = f.ty.base else {
            continue;
        };
        for m in &f.modifiers {
            let Modifier::Default(DefaultVal::Lit(lit)) = m else {
                continue;
            };
            match prim {
                Primitive::Time if !matches!(lit, Literal::Str(_) | Literal::Null) => {
                    sink.error(
                        code::TIME_DEFAULT,
                        f.span,
                        format!(
                            "default for time column `{}` must be a time string literal \
                             (e.g. \"14:30:00\")",
                            f.name.node
                        ),
                    );
                }
                Primitive::Bytes if !matches!(lit, Literal::Null) => {
                    sink.error(
                        code::BYTES_DEFAULT,
                        f.span,
                        format!(
                            "a bytes column (`{}`) cannot have a literal default — set it \
                             from a raw migration or a DB default",
                            f.name.node
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}
