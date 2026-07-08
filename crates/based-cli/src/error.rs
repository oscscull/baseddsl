//! The CLI's top-level error.
//!
//! One structured error every command handler returns, so `main` can print a clean,
//! actionable message and pick a process exit code by class: a usage/config mistake the
//! caller must fix (exit 2, matching clap's own arg-parse exit) versus an operational
//! failure the command hit while running (exit 1). Diagnostics for parse/sema errors are
//! still framed rustc-style by `render`; those paths set `summary_only` so this prints
//! just the trailing summary rather than a second, blobby copy.

use based_runtime::migrate::MigrateError;
use based_runtime::run::DbError;
use std::path::Path;
use std::process::ExitCode;

/// A CLI error: the message the caller reads on stderr, its exit class, an optional
/// underlying cause, and whether detail was already printed (so `report` stays terse).
pub struct CliError {
    kind: Kind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
    summary_only: bool,
}

/// The exit class: a mistake the caller must fix vs. a failure the run hit.
#[derive(Clone, Copy)]
enum Kind {
    /// A usage or configuration mistake (bad manifest, missing database url, unknown
    /// dialect, a schema that didn't check, a destructive migration needing a flag).
    Usage,
    /// An operational failure while running (database unreachable, io, migration failed).
    Failure,
}

impl CliError {
    /// A usage/config mistake the caller must fix. Exit code 2.
    pub fn usage(message: impl Into<String>) -> CliError {
        CliError {
            kind: Kind::Usage,
            message: message.into(),
            source: None,
            summary_only: false,
        }
    }

    /// An operational failure while running the command. Exit code 1.
    pub fn failure(message: impl Into<String>) -> CliError {
        CliError {
            kind: Kind::Failure,
            message: message.into(),
            source: None,
            summary_only: false,
        }
    }

    /// A summary line for a command whose per-item detail is already on stderr (the
    /// rustc-style diagnostics, or a `verify`/`status` problem list). Printed as-is.
    pub fn summary(kind_usage: bool, message: impl Into<String>) -> CliError {
        CliError {
            kind: if kind_usage {
                Kind::Usage
            } else {
                Kind::Failure
            },
            message: message.into(),
            source: None,
            summary_only: true,
        }
    }

    /// A filesystem failure, naming what was being read/written.
    pub fn io(
        context: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> CliError {
        CliError {
            kind: Kind::Failure,
            message: context.into(),
            source: Some(Box::new(source)),
            summary_only: false,
        }
    }

    /// An operational failure with an underlying cause preserved in the chain.
    pub fn caused_by(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> CliError {
        CliError {
            kind: Kind::Failure,
            message: message.into(),
            source: Some(Box::new(source)),
            summary_only: false,
        }
    }

    /// A migration failure. A destructive step needing `--allow-destructive` is the
    /// caller's to fix (usage); everything else is operational. The typed error is kept
    /// in the chain, so its `Display` (and any wrapped `DbError`) reads through.
    pub fn migrate(context: impl Into<String>, source: MigrateError) -> CliError {
        let kind = match source {
            MigrateError::Destructive { .. } => Kind::Usage,
            _ => Kind::Failure,
        };
        CliError {
            kind,
            message: context.into(),
            source: Some(Box::new(source)),
            summary_only: false,
        }
    }

    /// A database failure, carrying the driver's message + machine code in the chain.
    pub fn db(context: impl Into<String>, source: DbError) -> CliError {
        CliError {
            kind: Kind::Failure,
            message: context.into(),
            source: Some(Box::new(source)),
            summary_only: false,
        }
    }

    /// Print to stderr, then hand back the process exit code. A summary-only error is its
    /// message verbatim (detail already shown); otherwise a `based:` line plus the cause
    /// chain, one indented line each.
    pub fn report(&self) -> ExitCode {
        if self.summary_only {
            eprintln!("{}", self.message);
        } else {
            eprintln!("based: {}", self.message);
            let mut cause: Option<&(dyn std::error::Error + 'static)> =
                self.source.as_deref().map(|e| e as &dyn std::error::Error);
            while let Some(err) = cause {
                eprintln!("  caused by: {err}");
                cause = err.source();
            }
        }
        match self.kind {
            Kind::Usage => ExitCode::from(2),
            Kind::Failure => ExitCode::from(1),
        }
    }
}

/// Wrap an io failure against a path with a "reading/writing …" context.
pub fn io_at(action: &str, path: &Path, source: std::io::Error) -> CliError {
    CliError::io(format!("{action} {}", path.display()), source)
}
