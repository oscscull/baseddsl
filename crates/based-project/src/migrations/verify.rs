//! The offline migration gate shared by `based migrate verify` and `based-build`: every
//! `up.mig` matches its `schema.snap` chain, scripts plan for the dialect, the numbering
//! is gap-free, and the latest snapshot captures the current schema.
use super::MigrationSource;
use based_codegen::migrate::{self, Snapshot, Step};
use based_codegen::Dialect;
use based_manifest::Manifest;
use based_sema::CheckedSchema;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Fails the gate.
    Problem,
    /// Worth surfacing; does not fail the gate.
    Warning,
    /// Informational.
    Note,
}

/// One verification result, scoped to a migration id or to the whole set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub migration: Option<String>,
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.migration {
            Some(id) => write!(f, "{id}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Verify `migrations` (ordered by number) against the checked `schema`. Never touches a
/// database; the gate fails when any finding is a [`Severity::Problem`].
pub fn verify_migrations(
    schema: &CheckedSchema,
    manifest: &Manifest,
    migrations: &[MigrationSource<'_>],
) -> Vec<Finding> {
    let dialect = Dialect::parse(&manifest.dialect);
    let mut out = Vec::new();
    let mut prev = Snapshot::default();
    for (idx, source) in migrations.iter().enumerate() {
        let mut found = |severity, message: String| {
            out.push(Finding {
                migration: Some(source.id.to_string()),
                severity,
                message,
            });
        };
        let expected = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        if source.number() != Some(expected) {
            found(
                Severity::Problem,
                format!("number out of sequence (expected {expected:04})"),
            );
        }
        let snap = match Snapshot::parse(source.snapshot) {
            Ok(snap) => snap,
            Err(e) => {
                found(Severity::Problem, e.to_string());
                continue;
            }
        };
        let steps = migrate::up_steps(&prev, &snap, source.up);
        check_up_mig(source.up, &steps, &snap, &mut found);
        match migrate::plan_up(&prev, &snap, source.up, dialect) {
            Ok(plan) if plan.has_data => found(Severity::Note, "data steps checked".into()),
            Ok(_) => {}
            Err(problems) => {
                for p in &problems {
                    found(Severity::Problem, p.to_string());
                }
            }
        }
        if let Some(down) = source.down {
            if let Err(problems) = migrate::plan_down(&prev, &snap, down, dialect) {
                for p in &problems {
                    found(Severity::Problem, p.to_string());
                }
            }
        }
        prev = snap;
    }

    // Compared via the diff, not raw equality, so a spent `@was` still lingering in the
    // `.bsl` reads as no change.
    let fks = based_sema::ForeignKeys::parse(&manifest.schema.foreign_keys);
    let current = Snapshot::from_schema_with(schema, fks);
    let uncaptured = if migrations.is_empty() {
        (!current.tables.is_empty())
            .then_some("no migrations yet — run `based migrate gen` to capture the schema")
    } else {
        (!migrate::diff_snapshots(&prev, &current).is_empty()).then_some(
            "schema has uncaptured changes not in any migration — run `based migrate gen`",
        )
    };
    if let Some(message) = uncaptured {
        out.push(Finding {
            migration: None,
            severity: Severity::Problem,
            message: message.into(),
        });
    }
    out
}

/// A structural `up.mig` line that drifted from `schema.snap` is a problem; a `raw` step
/// makes the migration only partially verifiable, and one touching a modeled table is a
/// warning because the snapshot cannot see its effect.
fn check_up_mig(
    stored: &str,
    steps: &[Step],
    snap: &Snapshot,
    found: &mut impl FnMut(Severity, String),
) {
    if !migrate::up_mig_matches_snapshot(stored, steps) {
        found(
            Severity::Problem,
            "up.mig has drifted from schema.snap (re-run `based migrate gen`)".into(),
        );
    } else if migrate::has_raw_step(stored) {
        found(
            Severity::Note,
            "partial (carries a raw step — not offline-verifiable)".into(),
        );
    }
    for step in migrate::parse_raw_steps(stored) {
        if let Step::Raw { sql, .. } = &step {
            let touched = migrate::raw_modeled_tables(sql, snap);
            if !touched.is_empty() {
                found(
                    Severity::Warning,
                    format!(
                        "{} raw step touches modeled table(s) {} — the snapshot is blind to it",
                        based_sema::code::RAW_MIGRATION_MODELED,
                        touched.join(", ")
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAP: &str = "snapshot v1 dialect=neutral\n\ntable item\n  column id serial not_null\n  column name text not_null\n";
    const UP: &str =
        "format 2\ncreate table item {\n  column id serial not_null\n  column name text not_null\n}\n";

    fn verify(schema: &str, migrations: &[MigrationSource<'_>]) -> Vec<Finding> {
        let checked = crate::check_embedded("dialect = \"sqlite\"\n", &[("item.bsl", schema)])
            .expect("schema checks");
        verify_migrations(&checked.schema, &checked.project.manifest, migrations)
    }

    fn problems(findings: &[Finding]) -> Vec<String> {
        findings
            .iter()
            .filter(|f| f.severity == Severity::Problem)
            .map(ToString::to_string)
            .collect()
    }

    const INIT: MigrationSource<'static> = MigrationSource {
        id: "0001_init",
        up: UP,
        down: None,
        snapshot: SNAP,
    };

    #[test]
    fn a_captured_schema_verifies_clean() {
        let findings = verify("Item { id: serial, name: text }", &[INIT]);
        assert!(problems(&findings).is_empty(), "{findings:?}");
    }

    #[test]
    fn drift_gaps_and_uncaptured_changes_are_problems() {
        let drifted = MigrationSource {
            up: &UP.replace("column name", "column title"),
            ..INIT
        };
        let found = problems(&verify("Item { id: serial, name: text }", &[drifted]));
        assert!(
            found
                .iter()
                .any(|p| p.starts_with("0001_init: up.mig has drifted")),
            "{found:?}"
        );

        let gap = MigrationSource {
            id: "0002_init",
            ..INIT
        };
        let found = problems(&verify("Item { id: serial, name: text }", &[gap]));
        assert!(
            found.iter().any(|p| p.contains("expected 0001")),
            "{found:?}"
        );

        let found = problems(&verify(
            "Item { id: serial, name: text, note: text? }",
            &[INIT],
        ));
        assert!(
            found.iter().any(|p| p.contains("uncaptured changes")),
            "{found:?}"
        );

        let found = problems(&verify("Item { id: serial, name: text }", &[]));
        assert!(
            found.iter().any(|p| p.contains("no migrations yet")),
            "{found:?}"
        );
    }
}
