//! Fail the build on the same migration problems as `based migrate verify`.
use crate::Error;
use based_project::{CheckedProject, Finding, MigrationFiles, Severity};
use std::collections::BTreeMap;

pub(super) fn verify(checked: &CheckedProject, migrations: &[MigrationFiles]) -> Result<(), Error> {
    let sources: Vec<_> = migrations.iter().map(MigrationFiles::source).collect();
    let findings =
        based_project::verify_migrations(&checked.schema, &checked.project.manifest, &sources);
    let (problems, warnings): (Vec<_>, Vec<_>) = findings
        .into_iter()
        .filter(|f| f.severity != Severity::Note)
        .partition(|f| f.severity == Severity::Problem);
    for line in warning_lines(&warnings) {
        println!("cargo:warning={line}");
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Migrations(problems))
    }
}

/// Coded warnings collapse to one summary line per code, since they recur on every build
/// for migrations that are already applied and immutable; `based migrate verify` lists them.
fn warning_lines(warnings: &[Finding]) -> Vec<String> {
    let mut by_code: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut lines = Vec::new();
    for warning in warnings {
        match warning.code {
            Some(code) => by_code.entry(code).or_default().push(warning),
            None => lines.push(format!("migrations/{warning}")),
        }
    }
    for (code, group) in by_code {
        let mut ids: Vec<&str> = group
            .iter()
            .filter_map(|f| f.migration.as_deref())
            .collect();
        ids.dedup();
        let line = match group.as_slice() {
            [only] => format!("migrations/{only}"),
            _ => format!(
                "migrations: {code} in {} migration(s) — run `based migrate verify` for details",
                ids.len()
            ),
        };
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warning(migration: &str, code: Option<&'static str>, message: &str) -> Finding {
        Finding {
            migration: Some(migration.into()),
            severity: Severity::Warning,
            code,
            message: message.into(),
        }
    }

    #[test]
    fn repeated_coded_warnings_collapse_to_one_line() {
        let warnings = [
            warning(
                "0009_a",
                Some("W0109"),
                "raw step touches modeled table(s) a",
            ),
            warning(
                "0016_b",
                Some("W0109"),
                "raw step touches modeled table(s) b",
            ),
            warning(
                "0016_b",
                Some("W0109"),
                "raw step touches modeled table(s) c",
            ),
            warning("0018_c", None, "something else"),
        ];
        assert_eq!(
            warning_lines(&warnings),
            [
                "migrations/0018_c: something else",
                "migrations: W0109 in 2 migration(s) — run `based migrate verify` for details",
            ]
        );
    }

    #[test]
    fn a_single_coded_warning_keeps_its_detail() {
        let warnings = [warning(
            "0040_x",
            Some("W0109"),
            "raw step touches modeled table(s) x",
        )];
        assert_eq!(
            warning_lines(&warnings),
            ["migrations/0040_x: W0109 raw step touches modeled table(s) x"]
        );
    }
}
