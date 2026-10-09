mod artifacts;
mod benchmark;
mod cargo_generation;
mod client;
mod command;
mod consumer;
mod files;
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Options {
    #[command(subcommand)]
    task: Task,
}
#[derive(Subcommand)]
enum Task {
    GeneratedConsumer { dialect: String },
    RuntimeBenchmark,
    CargoGeneration,
}
#[tokio::main]
async fn main() -> Result<()> {
    match Options::parse().task {
        Task::GeneratedConsumer { dialect } => consumer::verify(&dialect),
        Task::RuntimeBenchmark => benchmark::verify(),
        Task::CargoGeneration => cargo_generation::verify(),
    }
}
