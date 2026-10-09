use based_catalog::{NativeType, QualifiedName, TypeFamily};
use sqlx::{
    postgres::{PgConnection, PgRow},
    Row,
};

pub(crate) async fn read(conn: &mut PgConnection, row: &PgRow) -> Result<NativeType, sqlx::Error> {
    let kind: String = row.try_get("type_kind")?;
    let name: String = row.try_get("type_name")?;
    let namespace: String = row.try_get("type_namespace")?;
    let category: String = row.try_get("category")?;
    let family = family(&namespace, &name, &kind, &category);
    let mut native = NativeType::declared(row.try_get::<String, _>("declaration")?, family);
    if matches!(family, TypeFamily::Enum | TypeFamily::Domain) {
        native.named_type = Some(QualifiedName::new(namespace, name.clone()));
    }
    if family == TypeFamily::Enum {
        native.enum_values = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT enumlabel::text FROM pg_catalog.pg_enum WHERE enumtypid = $1::bigint::oid ORDER BY enumsortorder"))
            .bind(row.try_get::<i64, _>("type_oid")?).fetch_all(conn).await?;
    }
    super::type_modifiers::apply(&mut native, &name, row.try_get("atttypmod")?);
    Ok(native)
}

fn family(namespace: &str, name: &str, kind: &str, category: &str) -> TypeFamily {
    match (kind, category) {
        ("e", _) => return TypeFamily::Enum,
        ("d", _) => return TypeFamily::Domain,
        (_, "A") => return TypeFamily::Array,
        _ => {}
    }
    if namespace != "pg_catalog" {
        return TypeFamily::Other;
    }
    match name {
        "bool" => TypeFamily::Boolean,
        "int2" | "int4" | "int8" => TypeFamily::Integer,
        "float4" | "float8" => TypeFamily::Real,
        "numeric" => TypeFamily::Decimal,
        "text" | "varchar" | "bpchar" => TypeFamily::Text,
        "bytea" => TypeFamily::Binary,
        "uuid" => TypeFamily::Uuid,
        "date" => TypeFamily::Date,
        "time" | "timetz" => TypeFamily::Time,
        "timestamp" | "timestamptz" => TypeFamily::Timestamp,
        "json" | "jsonb" => TypeFamily::Json,
        _ => TypeFamily::Other,
    }
}
