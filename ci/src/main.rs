mod artifacts;
mod benchmark;
mod cargo_generation;
mod client;
mod command;
mod consumer;
mod files;
mod import;
mod initializer;
mod onboarding;
mod release;
mod source;
mod source_consumer;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
struct Options {
    #[command(subcommand)]
    task: Task,
}
#[derive(Subcommand)]
enum Task {
    GeneratedConsumer {
        dialect: String,
    },
    RuntimeBenchmark,
    CargoGeneration,
    SourceConsumer {
        #[arg(long)]
        based: PathBuf,
    },
    Package {
        #[arg(long)]
        binary_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        target: String,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        allow_dirty: bool,
    },
    Collect {
        directory: PathBuf,
        #[arg(long, default_value_t = 5)]
        expected_count: usize,
    },
    Extension {
        directory: PathBuf,
        output: PathBuf,
    },
    Initializer {
        #[arg(long)]
        based: PathBuf,
    },
    EmbeddedTutorial {
        #[arg(long)]
        based: PathBuf,
    },
    ImportConsumer {
        #[arg(long)]
        based: PathBuf,
    },
    ImportServerConsumer {
        #[arg(long)]
        based: PathBuf,
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        dialect: String,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    match Options::parse().task {
        Task::GeneratedConsumer { dialect } => consumer::verify(&dialect),
        Task::RuntimeBenchmark => benchmark::verify(),
        Task::CargoGeneration => cargo_generation::verify(),
        Task::SourceConsumer { based } => source_consumer::verify(&based),
        Task::Package {
            binary_dir,
            output,
            target,
            tag,
            allow_dirty,
        } => release::package(&binary_dir, &output, &target, tag.as_deref(), allow_dirty).await,
        Task::Collect {
            directory,
            expected_count,
        } => release::collect(&directory, expected_count),
        Task::Extension { directory, output } => release::extension(&directory, &output),
        Task::Initializer { based } => initializer::verify(&based).await,
        Task::EmbeddedTutorial { based } => onboarding::verify(&based).await,
        Task::ImportConsumer { based } => import::sqlite(&based).await,
        Task::ImportServerConsumer {
            based,
            project,
            dialect,
        } => import::server(&based, &project, &dialect),
    }
}
