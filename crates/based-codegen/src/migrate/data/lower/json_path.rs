//! A dot path into a `json` column (`data.legal.commander`), read per dialect and cast to the
//! column family it is written to or compared with.

use super::super::super::sql::neutral_sql_type;
use super::super::table::Fam;
use crate::Dialect;

/// The value at `keys` inside the json column `col` (already quoted), cast for `to` (`None`
/// = its text).
pub(crate) fn json_path(
    dialect: Dialect,
    col: &str,
    keys: &[&str],
    to: Option<(&Fam, &str)>,
) -> String {
    let dollar = format!("'$.{}'", keys.join("."));
    let text = match dialect {
        Dialect::Sqlite => format!("json_extract({col}, {dollar})"),
        Dialect::MariaDb | Dialect::MySql => {
            format!("JSON_UNQUOTE(JSON_EXTRACT({col}, {dollar}))")
        }
        Dialect::Postgres => format!("({col} #>> '{{{}}}')", keys.join(",")),
    };
    let Some((fam, ty)) = to else { return text };
    match (fam, dialect) {
        (Fam::Text | Fam::StrEnum(_) | Fam::Ulid, _) => text,
        (Fam::Json, Dialect::Sqlite) => text,
        (Fam::Json, Dialect::MariaDb | Dialect::MySql) => {
            format!("JSON_EXTRACT({col}, {dollar})")
        }
        (Fam::Json, Dialect::Postgres) => format!("({col} #> '{{{}}}')", keys.join(",")),
        (Fam::Bool, Dialect::Sqlite) => text,
        (Fam::Bool, Dialect::MariaDb | Dialect::MySql) => {
            format!("(JSON_EXTRACT({col}, {dollar}) = CAST('true' AS JSON))")
        }
        (Fam::Uuid, Dialect::Postgres) => format!("{text}::uuid"),
        (Fam::Uuid, _) => text,
        _ => cast(dialect, &text, fam, ty),
    }
}

/// `CAST(text AS <type>)` for a numeric or temporal family.
fn cast(dialect: Dialect, text: &str, fam: &Fam, ty: &str) -> String {
    let target = match (fam, dialect) {
        (Fam::Int | Fam::IntEnum(_), Dialect::Sqlite) => "INTEGER".to_string(),
        (Fam::Int | Fam::IntEnum(_), Dialect::MariaDb | Dialect::MySql) => "SIGNED".to_string(),
        (Fam::Int | Fam::IntEnum(_), Dialect::Postgres) => "BIGINT".to_string(),
        (Fam::Float, Dialect::Sqlite) => "REAL".to_string(),
        (Fam::Float, Dialect::MariaDb | Dialect::MySql) => "DOUBLE".to_string(),
        (Fam::Float, Dialect::Postgres) => "DOUBLE PRECISION".to_string(),
        (Fam::Bool, _) => "BOOLEAN".to_string(),
        (Fam::Timestamp, Dialect::MariaDb | Dialect::MySql) => "DATETIME".to_string(),
        (Fam::Timestamp, Dialect::Postgres) => "TIMESTAMP".to_string(),
        (Fam::Date, Dialect::MariaDb | Dialect::MySql | Dialect::Postgres) => "DATE".to_string(),
        (Fam::Time, Dialect::MariaDb | Dialect::MySql | Dialect::Postgres) => "TIME".to_string(),
        // SQLite keeps temporal values as text.
        (Fam::Timestamp | Fam::Date | Fam::Time, Dialect::Sqlite) => return text.to_string(),
        _ => neutral_sql_type(ty, dialect),
    };
    format!("CAST({text} AS {target})")
}
