//! `based` — the compiler driver.
//!
//! `based check`: discover `.bsl` files -> parse -> sema -> render diagnostics.
//! `based gen sql`: the same front end, then emit SQL DDL from the checked schema.
//! `based gen client`: a typed Rust client module. `based gen openapi`: an OpenAPI 3.1
//! spec over the same wire (polyglot clients via `openapi-generator`).
//!
//! This entry parses arguments and dispatches to the per-command-group handlers; each
//! group lives in its own sibling module.

mod check;
mod error;
mod gen;
mod migrate;
mod project;
mod render;
mod serve;

use clap::{Parser, Subcommand};
use error::CliError;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "based", version = based_version::LONG, about = "based DSL compiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse + typecheck the project, print diagnostics.
    Check {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Format the project's `.bsl` files in the canonical layout.
    Fmt {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Don't write; exit nonzero if any file is not already formatted.
        #[arg(long)]
        check: bool,
    },
    /// Generate target artifacts from the checked schema.
    Gen {
        #[command(subcommand)]
        target: GenTarget,
    },
    /// Show the engine-derived facts (inferred inverses + indexes).
    Facts {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Emit machine-readable JSON instead of the human-readable listing.
        #[arg(long)]
        json: bool,
    },
    /// Generate + manage schema migrations (snapshot + diff, offline).
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
    /// Serve the checked schema as a live RPC service (`POST /q|m/<name>`).
    Serve {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Address to bind the HTTP listener on. `BASED_LISTEN` overrides the default —
        /// a container sets `0.0.0.0:8080` there so the port is reachable from outside.
        #[arg(long, env = "BASED_LISTEN", default_value = "127.0.0.1:8080")]
        listen: String,
        /// A database URL per physical shard (repeat for a sharded fleet). Falls back
        /// to `BASED_DATABASE_URL` (comma-separated) when none is passed.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
        /// Warm connections kept per shard pool.
        #[arg(long, default_value_t = 4)]
        pool_min: usize,
        /// Max connections per shard pool (the per-box concurrency cap).
        #[arg(long, default_value_t = 32)]
        pool_max: usize,
    },
}

#[derive(Subcommand)]
enum MigrateAction {
    /// Diff the current `.bsl` against the latest `schema.snap` and write the next
    /// `migrations/NNNN_slug/{up.mig, schema.snap}`. No changes ⇒ writes nothing.
    /// Offline + deterministic — never touches a database.
    Gen {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A short label for the migration slug (snake-cased). When omitted, the slug
        /// is derived from the change (`init` for the first, else `schema_update`).
        name: Option<String>,
    },
    /// Render migrations' neutral `up.mig` steps to per-dialect SQL and print it — the
    /// review-the-SQL step. Offline: reads the stored `schema.snap`s, never a DB.
    Render {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A specific migration number (`NNNN`) to render. When omitted, renders every
        /// migration in order.
        #[arg(long)]
        number: Option<u32>,
        /// Override the target dialect (`mariadb`/`sqlite`/`postgres`). Defaults to the
        /// manifest dialect.
        #[arg(long)]
        dialect: Option<String>,
    },
    /// Apply pending migrations to a live database, each under one transaction with a
    /// `_based_migrations` ledger insert + tamper-hash check. Destructive steps require
    /// `--allow-destructive`. Applies to every `--database-url` (a sharded fleet migrates
    /// each shard).
    Apply {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A database URL per physical shard (repeat for a sharded fleet). Falls back to
        /// `BASED_DATABASE_URL` (comma-separated) when none is passed.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
        /// Vouch for destructive steps (drops / narrowing / new not-null-without-default /
        /// new unique). Without it, apply stops before the first destructive migration.
        #[arg(long)]
        allow_destructive: bool,
        /// Reconcile the applied set to exactly migrations `≤ N`: roll forward up to `N`,
        /// roll back (via `down.mig`) anything applied above it. `--to 0` rolls back all.
        #[arg(long)]
        to: Option<u32>,
        /// Roll back only the most-recently-applied migration (via its `down.mig`).
        #[arg(long, conflicts_with = "to")]
        down: bool,
    },
    /// Show applied vs. pending migrations, flagging any hash mismatch (an edited applied
    /// migration). Reads the ledger from a live database.
    Status {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// The database to read the ledger from (first shard). Falls back to
        /// `BASED_DATABASE_URL`.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
    },
    /// Offline CI gate: confirm each `up.mig` still matches its `schema.snap` (no hand-edit
    /// drift) and the latest snapshot matches the current `.bsl` (no uncaptured changes).
    Verify {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Subcommand)]
enum GenTarget {
    /// Emit SQL DDL (`CREATE TABLE …`) for the manifest dialect.
    Sql {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Emit a typed client module for the manifest client target.
    Client {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Also emit the in-process **embedded bridge**: an `Embedded` `Transport`
        /// over `based_runtime::Engine` plus `client::embedded(&engine)`, so an embedding
        /// build gets a working client with no hand-written bridge. The consuming crate
        /// must depend on based-runtime; a pure-wire client leaves this off.
        #[arg(long)]
        embedded: bool,
    },
    /// Emit an OpenAPI 3.1 spec for the wire — feed it to `openapi-generator` for a
    /// client in any language (polyglot via one contract, not N emitters).
    Openapi {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

// The binary owns the async runtime; front-end commands are sync and just run on it,
// execution commands (serve, migrate apply/status) await the runtime's futures.
#[tokio::main]
async fn main() -> ExitCode {
    // clap prints its own usage error + exits 2 before we get here; our commands return a
    // structured error so `main` can pick a clean message + exit class (2 usage, 1 failure).
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => e.report(),
    }
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Check { root } => check::cmd_check(&root),
        Command::Fmt { root, check } => check::cmd_fmt(&root, check),
        Command::Gen { target } => match target {
            GenTarget::Sql { root, out } => gen::cmd_gen_sql(&root, out.as_deref()),
            GenTarget::Client {
                root,
                out,
                embedded,
            } => gen::cmd_gen_client(&root, out.as_deref(), embedded),
            GenTarget::Openapi { root, out } => gen::cmd_gen_openapi(&root, out.as_deref()),
        },
        Command::Migrate { action } => match action {
            MigrateAction::Gen { root, name } => migrate::cmd_migrate_gen(&root, name.as_deref()),
            MigrateAction::Render {
                root,
                number,
                dialect,
            } => migrate::cmd_migrate_render(&root, number, dialect.as_deref()),
            MigrateAction::Apply {
                root,
                database_url,
                allow_destructive,
                to,
                down,
            } => migrate::cmd_migrate_apply(&root, database_url, allow_destructive, to, down).await,
            MigrateAction::Status { root, database_url } => {
                migrate::cmd_migrate_status(&root, database_url).await
            }
            MigrateAction::Verify { root } => migrate::cmd_migrate_verify(&root),
        },
        Command::Facts { root, json } => project::cmd_facts(&root, json),
        Command::Serve {
            root,
            listen,
            database_url,
            pool_min,
            pool_max,
        } => serve::cmd_serve(&root, &listen, database_url, pool_min, pool_max).await,
    }
}
