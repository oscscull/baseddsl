//! Bounded warm sampling and summary statistics; no performance pass/fail threshold.
use crate::{fixture, Error};
use serde::Serialize;
use sqlx::SqlitePool;
use std::{future::Future, hint::black_box, time::Instant};

#[derive(Serialize)]
pub struct Summary {
    pub samples: usize,
    pub median_us: f64,
    pub p95_us: f64,
    pub min_us: f64,
    pub max_us: f64,
    pub operations_per_second: f64,
}

impl Summary {
    pub fn from_samples(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        let count = values.len();
        Self {
            samples: count,
            median_us: (values[(count - 1) / 2] + values[count / 2]) / 2.0,
            p95_us: values[(count * 95).div_ceil(100).saturating_sub(1)],
            min_us: values[0],
            max_us: values[count - 1],
            operations_per_second: count as f64 * 1_000_000.0 / values.iter().sum::<f64>(),
        }
    }
}

pub async fn sample<F, Fut, T>(
    count: usize,
    write: bool,
    pool: &SqlitePool,
    mut operation: F,
) -> Result<Summary, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    for _ in 0..10 {
        black_box(operation().await?);
        if write {
            fixture::reset(pool).await?;
        }
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let start = Instant::now();
        let output = operation().await?;
        values.push(start.elapsed().as_secs_f64() * 1_000_000.0);
        // Drop the fully materialized output outside the latency interval for both paths.
        black_box(output);
        if write {
            fixture::reset(pool).await?;
        }
    }
    Ok(Summary::from_samples(values))
}

pub fn cpu<T>(count: usize, mut operation: impl FnMut() -> T) -> Summary {
    cpu_prepared(count, || (), |()| operation())
}

pub fn cpu_prepared<I, T>(
    count: usize,
    mut prepare: impl FnMut() -> I,
    mut operation: impl FnMut(I) -> T,
) -> Summary {
    let values = (0..count)
        .map(|_| {
            let input = prepare();
            let start = Instant::now();
            let output = operation(input);
            let micros = start.elapsed().as_secs_f64() * 1_000_000.0;
            black_box(output);
            micros
        })
        .collect();
    Summary::from_samples(values)
}
