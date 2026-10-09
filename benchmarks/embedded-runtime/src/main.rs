mod case;
mod conversion;
mod direct;
mod fixture;
mod measure;
mod preflight;
mod report;
mod trace;
mod workload;

#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}

use based_runtime::{id::SeqIdGen, Compiled, Engine, SqliteBackend};
use report::Report;
use std::{path::Path, time::Instant};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let count = samples()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let start = Instant::now();
    let compiled = Compiled::load(root).map_err(|error| format!("{error:?}"))?;
    let startup_schema_load_us = start.elapsed().as_secs_f64() * 1_000_000.0;
    let pool = fixture::pool().await?;
    let engine = Engine::new(
        compiled,
        SqliteBackend::from_pool(pool.clone()),
        SeqIdGen::default(),
    );
    let mut workloads = Vec::new();
    for (index, case) in case::CASES.into_iter().enumerate() {
        workloads.push(workload::run(index, case, count, root, &pool, &engine).await?);
    }
    let report = Report {
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        sqlite_version: sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&pool)
            .await?,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        connections: 1,
        startup_schema_load_us,
        workloads,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    pool.close().await;
    Ok(())
}

fn samples() -> Result<usize, Error> {
    let count = std::env::args()
        .nth(1)
        .map_or(Ok(200), |value| value.parse::<usize>())?;
    if !(5..=10_000).contains(&count) {
        return Err("samples must be between 5 and 10000".into());
    }
    Ok(count)
}
