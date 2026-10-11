use based_ast::Literal;

/// Whether a numeric default literal fits `decimal(precision, scale)` without rounding:
/// at most `precision - scale` integer digits and `scale` fractional digits.
pub(crate) fn decimal_fits(lit: &Literal, precision: u32, scale: u32) -> bool {
    let text = match lit {
        Literal::Int(n) => n.unsigned_abs().to_string(),
        Literal::Decimal(s) => s.trim_start_matches('-').to_string(),
        _ => return true,
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let whole = whole.trim_start_matches('0');
    whole.len() as u32 <= precision.saturating_sub(scale) && fraction.len() as u32 <= scale
}
