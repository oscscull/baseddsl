//! Run and summarize one verified pair of execution paths.
use crate::{case::Case, conversion, direct, measure, preflight, report::ResultRow, Error};
use based_runtime::Engine;
use sqlx::SqlitePool;
use std::path::Path;

pub async fn run(
    index: usize,
    case: Case,
    count: usize,
    root: &Path,
    pool: &SqlitePool,
    engine: &Engine,
) -> Result<ResultRow, Error> {
    let verified = preflight::verify(case, root, pool).await?;
    // Alternate path order between workloads; run multiple processes to assess noise.
    let (direct_sqlx, embedded) = if index.is_multiple_of(2) {
        let direct = measure::sample(count, case.is_write(), pool, || {
            direct::run(case, &verified.statements, pool)
        })
        .await?;
        let embedded =
            measure::sample(count, case.is_write(), pool, || case.embedded(engine)).await?;
        (direct, embedded)
    } else {
        let embedded =
            measure::sample(count, case.is_write(), pool, || case.embedded(engine)).await?;
        let direct = measure::sample(count, case.is_write(), pool, || {
            direct::run(case, &verified.statements, pool)
        })
        .await?;
        (direct, embedded)
    };
    Ok(ResultRow {
        case: format!("{case:?}"),
        statements: verified.statements.len(),
        evidence: verified.evidence,
        direct_sqlx,
        embedded,
        conversion: conversion::measure(case, &verified.output, count)?,
    })
}
