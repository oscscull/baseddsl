use sqlx::Row;

#[derive(serde::Serialize)]
struct ItemRow {
    id: String,
    name: String,
}

pub async fn rows(pool: sqlx::SqlitePool) -> serde_json::Value {
    // Same ordinary query/result semantics; no query macros or compile-time database.
    let rows = sqlx::query("SELECT id, name FROM item ORDER BY id ASC")
        .fetch_all(&pool)
        .await
        .unwrap();
    let values: Vec<_> = rows
        .iter()
        .map(|row| ItemRow {
            id: row.get("id"),
            name: row.get("name"),
        })
        .collect();
    serde_json::to_value(values).unwrap()
}
