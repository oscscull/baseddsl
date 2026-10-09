use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use std::fs;

pub fn verify() -> Result<()> {
    let project = command::root().join("benchmarks/embedded-runtime");
    let env = Environment::new();
    let sql = command::run(command::based(), &["gen", "sql"], &project, &env)?;
    let tables = sql
        .split("-- ============================== queries")
        .next()
        .unwrap();
    ensure!(
        normalize(tables) == normalize(&fs::read_to_string(project.join("schema.sql"))?),
        "benchmark schema drift"
    );
    crate::client::check(&command::based(), &project)?;
    Ok(())
}

fn normalize(sql: &str) -> Vec<String> {
    sql.lines()
        .filter(|line| !line.starts_with("--"))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}
