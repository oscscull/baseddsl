use based_catalog::ValueGeneration;
use sqlx::{postgres::PgRow, Row};

pub(crate) fn read(
    row: &PgRow,
    expression: Option<String>,
) -> Result<ValueGeneration, sqlx::Error> {
    match row.try_get::<String, _>("identity")?.as_str() {
        "a" => return Ok(ValueGeneration::Identity { always: true }),
        "d" => return Ok(ValueGeneration::Identity { always: false }),
        _ => {}
    }
    if row.try_get::<String, _>("generated")? == "s" {
        return Ok(ValueGeneration::Generated {
            expression,
            stored: true,
        });
    }
    // Retain the sequence expression exactly; never evaluate nextval or read sequence state.
    if let Some(expression) = expression.filter(|expression| expression.starts_with("nextval(")) {
        return Ok(ValueGeneration::Sequence { expression });
    }
    Ok(ValueGeneration::None)
}
