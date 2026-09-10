use super::*;


/// The offline drift diagnostics for a project: the schema-vs-migrations delta the editor
/// can compute with no database. Empty unless the project has captured
/// migrations *and* the current `.bsl` has structural changes not yet in one. Each change
/// is anchored at the declaration it touches (a model's name) and the whole set is
/// reported with the total count ("N uncaptured schema changes — run `based migrate gen`").
/// A spent `@was` (a rename already captured) is flagged separately so it can be removed.
pub(crate) fn drift_diagnostics(
    root: &Path,
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
    fks: based_sema::ForeignKeys,
) -> Vec<Diagnostic> {
    use based_codegen::migrate;
    // Only projects that have opted into migrations get a drift check — no `migrations/`
    // (or an unreadable latest snapshot) means the author isn't tracking migrations yet.
    let Some(prev) = latest_snapshot(root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let now = migrate::Snapshot::from_schema_with(schema, fks);
    let steps = migrate::diff_snapshots(&prev, &now);
    let total = steps.len();
    if total > 0 {
        // Group each step under the model it touches, so one diagnostic per changed
        // declaration lists its own changes; steps with no locatable model (a dropped
        // table, a scope-only change) fall back to the first model's name.
        let msg = format!(
            "{total} uncaptured schema change{} — run `based migrate gen`",
            if total == 1 { "" } else { "s" }
        );
        let mut by_span: HashMap<(u32, u32, u32), Vec<String>> = HashMap::new();
        let mut order: Vec<Span> = Vec::new();
        for step in &steps {
            let span = step
                .table_name()
                .and_then(|t| self_model_span(schema, decls, t))
                .or_else(|| first_model_span(decls));
            let Some(span) = span else { continue };
            let key = (span.file.0, span.start, span.end);
            if !by_span.contains_key(&key) {
                order.push(span);
            }
            by_span.entry(key).or_default().push(step.describe());
        }
        // Teach-at-checkpoint: a drop-one/add-one-same-family diff on a table is ambiguous
        // with a rename, so the drift note points at `@was`.
        for hint in migrate::rename_hints(&prev, &now) {
            if let Some(span) = self_model_span(schema, decls, &hint.table) {
                let key = (span.file.0, span.start, span.end);
                if !by_span.contains_key(&key) {
                    order.push(span);
                }
                by_span.entry(key).or_default().push(hint.message());
            }
        }
        for span in order {
            let key = (span.file.0, span.start, span.end);
            let note = by_span.remove(&key).unwrap_or_default().join("; ");
            out.push(
                Diagnostic::warning(based_sema::code::MIGRATE_DRIFT, msg.clone())
                    .at(span)
                    .note(note),
            );
        }
    }

    out.extend(spent_was_diagnostics(&prev, schema, decls));
    out
}


/// Spent `@was` directives: a rename already reflected in the latest snapshot (the old
/// name is gone, the new name present), so the `@was` no longer does anything and should
/// be removed. Anchored at the `@was` literal.
fn spent_was_diagnostics(
    prev: &based_codegen::migrate::Snapshot,
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in decls {
        let Decl::Model(m) = d else { continue };
        let Some(rmodel) = schema.model(&m.name.node) else {
            continue;
        };
        // Field-level `@was`.
        for mem in &m.members {
            let Member::Field(f) = mem else { continue };
            let Some(was) = &f.was else { continue };
            let new_col = rmodel
                .member(&f.name.node)
                .map(|rm| rm.physical_col().to_string());
            if let (Some(t), Some(new_col)) = (prev.table(&rmodel.table), new_col) {
                if t.column(&was.node).is_none() && t.column(&new_col).is_some() {
                    out.push(spent_was(was.span, &was.node));
                }
            }
        }
        // Model-level `@was("old_table")`.
        for deco in &m.decorators {
            if deco.name.node != "was" {
                continue;
            }
            let Some(based_ast::DecoArg::Lit(based_ast::Literal::Str(old))) = deco.args.first()
            else {
                continue;
            };
            if prev.table(old).is_none() && prev.table(&rmodel.table).is_some() {
                out.push(spent_was(deco.span, old));
            }
        }
    }
    out
}


fn spent_was(span: Span, old: &str) -> Diagnostic {
    Diagnostic::warning(
        based_sema::code::WAS_SPENT,
        format!("`@was(\"{old}\")` rename already captured — remove it"),
    )
    .at(span)
}


/// The declaration-name span of the model whose (physical) table is `table`.
fn self_model_span(
    schema: &based_sema::CheckedSchema,
    decls: &[Decl],
    table: &str,
) -> Option<Span> {
    let name = schema
        .models
        .iter()
        .find(|m| m.table == table)?
        .name
        .clone();
    decls.iter().find_map(|d| match d {
        Decl::Model(m) if m.name.node == name => Some(m.name.span),
        _ => None,
    })
}


/// The first model declaration's name span — a fallback anchor for a drift step whose
/// table no longer has a declaration (a dropped model).
fn first_model_span(decls: &[Decl]) -> Option<Span> {
    decls.iter().find_map(|d| match d {
        Decl::Model(m) => Some(m.name.span),
        _ => None,
    })
}


/// The highest-numbered `migrations/NNNN_slug/schema.snap` under `root`, parsed — the
/// diff baseline for the drift check. `None` when there is no `migrations/` dir, no
/// migration in it, or the snapshot is unreadable/corrupt (drift stays silent then).
pub(crate) fn latest_snapshot(root: &Path) -> Option<based_codegen::migrate::Snapshot> {
    let dir = root.join("migrations");
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((num, _)) = name.split_once('_') {
            if let Ok(n) = num.parse::<u32>() {
                if best.as_ref().is_none_or(|(b, _)| n > *b) {
                    best = Some((n, entry.path()));
                }
            }
        }
    }
    let (_, path) = best?;
    let text = std::fs::read_to_string(path.join("schema.snap")).ok()?;
    based_codegen::migrate::Snapshot::parse(&text).ok()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// A throwaway workspace dir under the system temp, removed on drop.
    /// An added column not yet in the latest `schema.snap` surfaces as an offline drift
    /// diagnostic anchored at the changed model.
    #[test]
    fn uncaptured_change_is_a_drift_diagnostic() {
        let ws = TempWorkspace::new("drift");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text?\n}\n",
        );
        // The captured snapshot knows only `name` — `barcode` is uncaptured.
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column name text not_null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");

        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            snap.diagnostics.iter().any(|d| d.code == "W0108"),
            "expected W0108 drift, got {:?}",
            snap.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }


    /// A drop-one/add-one-same-family diff (a rename spelled as drop+add) tags the
    /// drift note with the teach-`@was` hint, so the ambiguity is self-revealing.
    #[test]
    fn drift_note_teaches_was_on_a_drop_add_rename() {
        let ws = TempWorkspace::new("teach");
        ws.write("based.toml", "");
        // Source renames `upc` → `barcode` WITHOUT @was — an ambiguous drop+add.
        ws.write("schema.bsl", "Product {\n  barcode: text\n}\n");
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column upc text not_null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");

        let snap = compile_manifest(&ws.root, &HashMap::new());
        let w0108: Vec<_> = snap
            .diagnostics
            .iter()
            .filter(|d| d.code == "W0108")
            .collect();
        assert!(!w0108.is_empty(), "expected W0108 drift");
        assert!(
            w0108
                .iter()
                .any(|d| d.notes.iter().any(|n| n.contains("@was(\"upc\")"))),
            "drift note should teach @was: {:?}",
            w0108.iter().map(|d| &d.notes).collect::<Vec<_>>()
        );
    }


    /// When the snapshot already matches the schema, there is no drift diagnostic; and a
    /// spent `@was` (rename already captured) is surfaced.
    #[test]
    fn captured_schema_has_no_drift_but_flags_spent_was() {
        let ws = TempWorkspace::new("nodrift");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  barcode: text? @was(\"upc\")\n}\n",
        );
        // Snapshot already has `barcode` (the rename was captured) — no `upc` to rename.
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column barcode text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");

        let snap = compile_manifest(&ws.root, &HashMap::new());
        let codes: Vec<&str> = snap.diagnostics.iter().map(|d| d.code).collect();
        assert!(!codes.contains(&"W0108"), "unexpected drift: {codes:?}");
        assert!(
            codes.contains(&"W0107"),
            "expected spent-@was W0107: {codes:?}"
        );
    }

}
