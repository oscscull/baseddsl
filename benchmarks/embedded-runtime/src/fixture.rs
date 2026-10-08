//! Identical schema, data, and one connection shared by both execution paths.
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

use crate::{client, Error};

pub async fn pool() -> Result<SqlitePool, Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::raw_sql(include_str!("../schema.sql"))
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    for owner in 1..=128_i64 {
        sqlx::query("INSERT INTO owner(id, name) VALUES (?, ?)")
            .bind(owner)
            .bind(format!("owner-{owner}"))
            .execute(&mut *tx)
            .await?;
        for child in 0..8_i64 {
            let id = (owner - 1) * 8 + child + 1;
            sqlx::query("INSERT INTO item(id, owner_id, name, value) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind(owner)
                .bind(format!("item-{id}"))
                .bind(id * 3)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(pool)
}

pub fn bulk(size: usize) -> client::BulkInput {
    client::BulkInput {
        rows: (0..size)
            .map(|index| client::ItemInput {
                owner: client::ItemInputOwner {
                    id: client::Id::from_int(1),
                },
                name: format!("bulk-{index}"),
                value: index as i64,
            })
            .collect(),
    }
}

/// Outside every timed interval; restores both data and serial sequence after writes.
pub async fn reset(pool: &SqlitePool) -> Result<(), Error> {
    sqlx::query("DELETE FROM item WHERE id > 1024")
        .execute(pool)
        .await?;
    sqlx::query("UPDATE sqlite_sequence SET seq = 1024 WHERE name = 'item'")
        .execute(pool)
        .await?;
    Ok(())
}
