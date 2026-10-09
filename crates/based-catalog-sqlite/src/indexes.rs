use based_catalog::{
    CatalogCode, CatalogDiagnostic, CatalogSeverity, Index, IndexDirection, IndexOrigin, IndexPart,
    IndexTarget, TableId,
};
use sqlx::{Row, SqliteConnection};

pub(crate) async fn read(
    conn: &mut SqliteConnection,
    name: &str,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Vec<Index>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/indexes.sql")))
        .bind(name)
        .fetch_all(&mut *conn)
        .await?;
    let mut indexes = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let origin = match row.try_get::<String, _>("origin")?.as_str() {
            "pk" => IndexOrigin::PrimaryKey,
            "u" => IndexOrigin::UniqueConstraint,
            "c" => IndexOrigin::Explicit,
            _ => return Err(sqlx::Error::ColumnNotFound("index origin".into())),
        };
        let native_definition: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(
            "SELECT sql FROM main.sqlite_schema WHERE type = 'index' AND name = ?",
        ))
        .bind(&name)
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
        let parts = parts(conn, &name).await?;
        if row.try_get::<i64, _>("partial")? != 0 {
            findings.push(CatalogDiagnostic { table: id.clone(), member: Some(name.clone()), severity: CatalogSeverity::Error,
                code: CatalogCode::IncompleteMetadata, message: "Partial-index predicate is retained in native index DDL; individual normalization is unavailable".into() });
        }
        let predicate = None;
        indexes.push(Index {
            name,
            origin,
            unique: row.try_get::<i64, _>("is_unique")? != 0,
            parts,
            included_columns: Vec::new(),
            method: None,
            predicate,
            native_definition,
            valid: true,
        });
    }
    Ok(indexes)
}

async fn parts(conn: &mut SqliteConnection, name: &str) -> Result<Vec<IndexPart>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT cid, name, \"desc\" AS descending, coll FROM pragma_index_xinfo(?, 'main') WHERE \"key\" = 1 ORDER BY seqno"))
        .bind(name).fetch_all(conn).await?;
    rows.iter()
        .map(|row| {
            let target = match row.try_get::<i64, _>("cid")? {
                -2 => IndexTarget::Expression(None),
                number if number >= 0 => IndexTarget::Column(row.try_get("name")?),
                _ => return Err(sqlx::Error::ColumnNotFound("index rowid part".into())),
            };
            let direction = match row.try_get::<i64, _>("descending")? {
                0 => IndexDirection::Ascending,
                _ => IndexDirection::Descending,
            };
            Ok(IndexPart {
                target,
                direction,
                prefix_length: None,
                collation: row.try_get("coll")?,
            })
        })
        .collect()
}
