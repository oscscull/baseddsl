//! Result/statement parity and query-plan evidence before any timings are trusted.
use crate::{
    case::{Case, Output},
    direct, fixture,
    trace::{Capture, Statement},
    Error,
};
use based_runtime::{id::SeqIdGen, Compiled, Engine, SqliteBackend};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Serialize)]
pub struct Evidence {
    pub sql: String,
    pub binds: String,
    pub query_plan: Vec<String>,
}

pub struct Verified {
    pub statements: Vec<Statement>,
    pub output: Output,
    pub evidence: Vec<Evidence>,
}

pub async fn verify(case: Case, root: &Path, pool: &SqlitePool) -> Result<Verified, Error> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let compiled = Compiled::load(root).map_err(|error| format!("{error:?}"))?;
    let engine = Engine::new(
        compiled,
        Capture {
            inner: SqliteBackend::from_pool(pool.clone()),
            log: log.clone(),
        },
        SeqIdGen::default(),
    );
    let embedded = case.embedded(&engine).await?;
    validate_shape(case, &embedded);
    let statements = log.lock().unwrap().clone();
    let embedded_data = snapshot(case, pool).await?;
    if let Case::Bulk(size) = case {
        assert_eq!(embedded_data.len(), size);
    }
    if case.is_write() {
        fixture::reset(pool).await?;
    }
    let direct = direct::run(case, &statements, pool).await?;
    assert_eq!(
        serde_json::to_value(&embedded)?,
        serde_json::to_value(&direct)?,
        "result parity {case:?}"
    );
    assert_eq!(
        embedded_data,
        snapshot(case, pool).await?,
        "persisted write parity {case:?}"
    );
    if case.is_write() {
        fixture::reset(pool).await?;
    }
    let expected = match case {
        Case::Page => 2,
        Case::Bulk(size) => size.div_ceil(300),
        _ => 1,
    };
    assert_eq!(statements.len(), expected, "SQL statement count {case:?}");
    let evidence = explain(&statements, pool).await?;
    Ok(Verified {
        statements,
        output: embedded,
        evidence,
    })
}

fn validate_shape(case: Case, output: &Output) {
    match (case, output) {
        (Case::Flat(size), Output::Items(rows)) => {
            assert_eq!(rows.len(), size as usize);
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(row.id.as_str(), (index + 1).to_string());
                assert_eq!(row.name, format!("item-{}", index + 1));
                assert_eq!(row.value, (index as i64 + 1) * 3);
            }
        }
        (Case::Nested(size), Output::Owners(rows)) => {
            assert_eq!(rows.len(), size as usize);
            assert!(rows.iter().all(|row| row.items.len() == 8));
        }
        (Case::Page, Output::Page(page)) => {
            assert_eq!(page.rows.len(), 32);
            assert_eq!(page.rows[0].id.as_str(), "33");
            assert_eq!(page.total, Some(1024));
            assert!(page.cursor.is_none());
        }
        (Case::Bulk(_), Output::Ack(())) => {}
        _ => panic!("incorrect workload output"),
    }
}

async fn snapshot(case: Case, pool: &SqlitePool) -> Result<Vec<(i64, i64, String, i64)>, Error> {
    if !case.is_write() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as("SELECT id, owner_id, name, value FROM item WHERE id > 1024 ORDER BY id")
            .fetch_all(pool)
            .await?,
    )
}

async fn explain(statements: &[Statement], pool: &SqlitePool) -> Result<Vec<Evidence>, Error> {
    let mut evidence = Vec::new();
    for statement in statements {
        let explain = Statement {
            sql: format!("EXPLAIN QUERY PLAN {}", statement.sql),
            params: statement.params.clone(),
        };
        let rows = direct::query(&explain)?.fetch_all(pool).await?;
        evidence.push(Evidence {
            sql: statement.sql.clone(),
            binds: format!("{:?}", statement.params),
            query_plan: rows
                .iter()
                .map(|row| row.try_get("detail"))
                .collect::<Result<_, _>>()?,
        });
    }
    Ok(evidence)
}
