use based_catalog::{NativeType, TypeFamily};
use sqlx::{mysql::MySqlRow, Row};

pub(crate) fn read(row: &MySqlRow) -> Result<NativeType, sqlx::Error> {
    let declaration: String = row.try_get("COLUMN_TYPE")?;
    let data_type: String = row.try_get("DATA_TYPE")?;
    let mut native = NativeType::declared(&declaration, family(&data_type));
    native.unsigned = declaration
        .split_ascii_whitespace()
        .any(|part| part == "unsigned");
    native.length = row.try_get("CHARACTER_MAXIMUM_LENGTH")?;
    native.precision = row
        .try_get::<Option<u64>, _>("NUMERIC_PRECISION")?
        .map(u16::try_from)
        .transpose()
        .map_err(|_| sqlx::Error::ColumnNotFound("precision overflow".into()))?;
    native.scale = row
        .try_get::<Option<u64>, _>("NUMERIC_SCALE")?
        .map(i16::try_from)
        .transpose()
        .map_err(|_| sqlx::Error::ColumnNotFound("scale overflow".into()))?;
    native.charset = row.try_get("CHARACTER_SET_NAME")?;
    // MariaDB native TIMESTAMP/DATETIME are distinct declarations, not inferred time zones.
    Ok(native)
}

fn family(data_type: &str) -> TypeFamily {
    match data_type {
        "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => TypeFamily::Integer,
        "float" | "double" => TypeFamily::Real,
        "decimal" => TypeFamily::Decimal,
        "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" => TypeFamily::Text,
        "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => {
            TypeFamily::Binary
        }
        "uuid" => TypeFamily::Uuid,
        "date" => TypeFamily::Date,
        "time" => TypeFamily::Time,
        "datetime" | "timestamp" => TypeFamily::Timestamp,
        "enum" => TypeFamily::Enum,
        // SET, BIT, YEAR, geometry and extensions have no automatic conversion promise.
        _ => TypeFamily::Other,
    }
}
