//! Direct SQLx execution: identical captured SQL, native typed materialization.
use crate::{
    case::{Case, Output},
    client, fixture,
    trace::Statement,
    Error,
};
use based_runtime::value::SqlValue;
use sqlx::{
    query::Query,
    sqlite::{SqliteArguments, SqliteRow},
    Row, Sqlite, SqlitePool,
};

pub fn query(statement: &Statement) -> Result<Query<'_, Sqlite, SqliteArguments>, Error> {
    statement
        .params
        .iter()
        // SQL comes only from the checked compiler output; values remain bound.
        .try_fold(
            sqlx::query(sqlx::AssertSqlSafe(statement.sql.as_str())),
            |query, value| {
                Ok(match value {
                    SqlValue::Int(value) => query.bind(*value),
                    SqlValue::Text(value) => query.bind(value.clone()),
                    _ => return Err(format!("unsupported benchmark bind: {value:?}").into()),
                })
            },
        )
}

fn item(row: &SqliteRow) -> Result<client::ItemRow, Error> {
    Ok(client::ItemRow {
        id: client::Id::from_int(row.try_get::<i64, _>("id")?),
        name: row.try_get("name")?,
        value: row.try_get("value")?,
    })
}

fn owner(row: &SqliteRow) -> Result<client::OwnerRow, Error> {
    Ok(client::OwnerRow {
        id: client::Id::from_int(row.try_get::<i64, _>("id")?),
        name: row.try_get("name")?,
        // Identical SQL-side JSON aggregation; direct SQLx must decode it too.
        items: serde_json::from_str(row.try_get::<&str, _>("items[]")?)?,
    })
}

pub async fn run(case: Case, statements: &[Statement], pool: &SqlitePool) -> Result<Output, Error> {
    if let Case::Bulk(size) = case {
        return bulk(size, statements, pool).await;
    }
    let mut connection = pool.acquire().await?;
    let rows = query(&statements[0])?.fetch_all(&mut *connection).await?;
    match case {
        Case::Nested(_) => Ok(Output::Owners(
            rows.iter().map(owner).collect::<Result<_, _>>()?,
        )),
        Case::Page => {
            let count = query(&statements[1])?.fetch_one(&mut *connection).await?;
            Ok(Output::Page(client::Page {
                rows: rows.iter().map(item).collect::<Result<_, _>>()?,
                cursor: None,
                total: Some(count.try_get("count")?),
            }))
        }
        Case::Flat(_) => Ok(Output::Items(
            rows.iter().map(item).collect::<Result<_, _>>()?,
        )),
        Case::Bulk(_) => unreachable!(),
    }
}

async fn bulk(size: usize, statements: &[Statement], pool: &SqlitePool) -> Result<Output, Error> {
    let input = fixture::bulk(size);
    let mut tx = pool.begin().await?;
    // Runtime's SQLite budget: 900 binds, three columns, 300 rows per statement.
    assert_eq!(statements.len(), input.rows.len().div_ceil(300));
    for (statement, rows) in statements.iter().zip(input.rows.chunks(300)) {
        let values = vec!["(?, ?, ?)"; rows.len()].join(", ");
        let sql = format!("INSERT INTO `item` (`owner_id`, `name`, `value`)\nVALUES {values};\n");
        assert_eq!(sql, statement.sql, "direct bulk must execute identical SQL");
        // Only a fixed literal and a count of placeholder groups form this SQL.
        let query = rows.iter().try_fold(
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str())),
            |query, row| {
                Ok::<_, Error>(
                    query
                        .bind(row.owner.id.as_str().parse::<i64>()?)
                        .bind(row.name.clone())
                        .bind(row.value),
                )
            },
        )?;
        query.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Output::Ack(()))
}
