#[cfg(feature = "embedded")]
use based_runtime::sqlx;

pub async fn seed() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE item (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO item VALUES ('00000000-0000-4000-8000-000000000001', 'Ada')")
        .execute(&pool)
        .await
        .unwrap();
    pool
}
