mod client;
mod command;
mod consumer;
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
}
#[tokio::main]
async fn main() -> Result<()> {
    match Options::parse().task {
        Task::GeneratedConsumer { dialect } => consumer::verify(&dialect),
    }
}
