//! The transaction a SQLite table-rebuild migration runs in.
//!
//! A rebuild renames the table away, recreates it and drops the old copy. With
//! `foreign_keys` on, SQLite re-points every child's `REFERENCES` at the renamed copy and
//! the drop cascades into the children, so SQLite's documented procedure ("Making Other
//! Kinds Of Table Schema Changes") turns foreign keys off around the transaction and runs
//! `foreign_key_check` before committing. The pragma is a no-op inside a transaction, so
//! this wrapper owns the connection and issues `BEGIN` / `COMMIT` itself.

use async_trait::async_trait;
use based_codegen::migrate::SQLITE_REBUILD_OPEN;
use based_codegen::Dialect;

use crate::run::{fetch_all, Db, DbError, DbRead, Row, RowStream, Tx};
use crate::value::SqlValue;

/// Whether a migration's statements include a SQLite table rebuild.
pub(super) fn has_rebuild<'a>(dialect: Dialect, mut sqls: impl Iterator<Item = &'a str>) -> bool {
    dialect == Dialect::Sqlite && sqls.any(|s| s.trim().eq_ignore_ascii_case(SQLITE_REBUILD_OPEN))
}

/// Open the migration's transaction on `db`. A rebuild on a connection enforcing foreign
/// keys gets the foreign-keys-off procedure; anything else gets the driver's own `begin`.
pub(super) async fn begin(db: Box<dyn Db>, rebuild: bool) -> Result<Box<dyn Tx>, DbError> {
    if !rebuild {
        return db.begin().await;
    }
    let mut db = db;
    if !enforcing(&mut *db).await? {
        return db.begin().await;
    }
    db.execute("PRAGMA foreign_keys = OFF", &[]).await?;
    if let Err(e) = db.execute("BEGIN", &[]).await {
        return Err(restore(&mut *db).await.err().unwrap_or(e));
    }
    Ok(Box::new(ForeignKeysOff { db }))
}

/// Whether the connection currently enforces foreign keys.
async fn enforcing(db: &mut dyn Db) -> Result<bool, DbError> {
    let rows = fetch_all(db.fetch("PRAGMA foreign_keys", &[])).await?;
    Ok(rows
        .first()
        .and_then(|r| r.values().next())
        .and_then(serde_json::Value::as_i64)
        == Some(1))
}

async fn restore(db: &mut dyn Db) -> Result<(), DbError> {
    db.execute("PRAGMA foreign_keys = ON", &[])
        .await
        .map(|_| ())
}

/// A transaction opened with foreign keys off. Commit and rollback both turn them back on.
struct ForeignKeysOff {
    db: Box<dyn Db>,
}

impl ForeignKeysOff {
    /// Roll back, restore enforcement, and report `cause`.
    async fn abandon(mut self, cause: DbError) -> DbError {
        let _ = self.db.execute("ROLLBACK", &[]).await;
        restore(&mut *self.db).await.err().unwrap_or(cause)
    }
}

#[async_trait]
impl DbRead for ForeignKeysOff {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        self.db.fetch(sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.db.execute(sql, params).await
    }
}

#[async_trait]
impl Tx for ForeignKeysOff {
    async fn commit(mut self: Box<Self>) -> Result<(), DbError> {
        let violations = match fetch_all(self.db.fetch("PRAGMA foreign_key_check", &[])).await {
            Ok(rows) => rows,
            Err(e) => return Err(self.abandon(e).await),
        };
        if !violations.is_empty() {
            return Err(self.abandon(violation_error(&violations)).await);
        }
        if let Err(e) = self.db.execute("COMMIT", &[]).await {
            return Err(self.abandon(e).await);
        }
        restore(&mut *self.db).await
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), DbError> {
        self.db.execute("ROLLBACK", &[]).await?;
        restore(&mut *self.db).await
    }
}

/// The `foreign_key_check` rows as one error: each child table and row whose parent is missing.
fn violation_error(rows: &[Row]) -> DbError {
    let field = |r: &Row, k: &str| match r.get(k) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    let listed: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{} row {} → {}",
                field(r, "table"),
                field(r, "rowid"),
                field(r, "parent")
            )
        })
        .collect();
    DbError::new(format!(
        "foreign key check failed after table rebuild: {}",
        listed.join(", ")
    ))
}
