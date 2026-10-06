//! Check or explicitly provision the idempotency table on each physical database.
use crate::error::CliError;
use crate::idempotency_store::StoreOptions;
use based_codegen::{sql, Dialect};
use based_runtime::{Backend, DbRead, SqliteBackend};

async fn prepare(
    db: &mut dyn DbRead,
    dialect: Dialect,
    options: StoreOptions,
) -> Result<(), CliError> {
    if options.init_idempotency_table {
        db.execute(&sql::idempotency_table_ddl(dialect), &[])
            .await
            .map_err(|e| {
                CliError::db(
                    "creating idempotency table (CREATE TABLE permission required)",
                    e,
                )
            })?;
    }
    let columns = ["callable", "key", "fingerprint", "response", "created_at"]
        .map(|name| dialect.quote(name))
        .join(", ");
    let table = dialect.quote(sql::IDEMPOTENCY_TABLE);
    let probe = format!("SELECT {columns} FROM {table} WHERE 1=0");
    based_runtime::run::fetch_all(db.fetch(&probe, &[])).await
        .map_err(|e| CliError::db("checking idempotency table: provision it on every shard or explicitly use --init-idempotency-table; local replay uses --idempotency-store memory", e))?;
    Ok(())
}

pub async fn sqlite(
    backend: &SqliteBackend,
    dialect: Dialect,
    options: StoreOptions,
) -> Result<(), CliError> {
    if !options.uses_database() {
        return Ok(());
    }
    let mut db = backend
        .checkout("")
        .await
        .map_err(|e| CliError::db("connecting to idempotency database", e))?;
    prepare(db.as_mut(), dialect, options).await
}

#[cfg(feature = "postgres")]
pub async fn postgres(
    router: &based_runtime::PgRouter,
    dialect: Dialect,
    options: StoreOptions,
) -> Result<(), CliError> {
    if !options.uses_database() {
        return Ok(());
    }
    for shard in 0..router.shard_count() {
        let mut db = router.checkout_shard(shard).await.map_err(|e| {
            CliError::db(
                format!("connecting to idempotency database shard {shard}"),
                e,
            )
        })?;
        prepare(&mut db, dialect, options).await?;
    }
    Ok(())
}

#[cfg(feature = "mariadb")]
pub async fn mariadb(
    router: &based_runtime::driver::ShardRouter,
    dialect: Dialect,
    options: StoreOptions,
) -> Result<(), CliError> {
    if !options.uses_database() {
        return Ok(());
    }
    for shard in 0..router.shard_count() {
        let mut db = router.checkout_shard(shard).await.map_err(|e| {
            CliError::db(
                format!("connecting to idempotency database shard {shard}"),
                e,
            )
        })?;
        prepare(&mut db, dialect, options).await?;
    }
    Ok(())
}
