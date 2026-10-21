use super::{label::param_label, positional::positional, CallableKind, Explained};
use crate::load::Compiled;
use crate::plan::find_mutation;

/// Each write in order (with any follow-up read-back), then the declared-shape re-select.
pub(super) fn explain(compiled: &Compiled, name: &str) -> Option<Explained> {
    let lowered = compiled.mutations.get(name)?;
    let params = find_mutation(&compiled.decls, name).map_or(&[][..], |m| &m.params[..]);
    let stmt = |label: &str, sql: &str| {
        positional(label, sql, compiled.dialect, |p| param_label(params, p))
    };
    let mut statements = Vec::new();
    for write in &lowered.stmts {
        statements.push(stmt(
            &write_label(&write.header, write.bulk.is_some()),
            &write.sql,
        ));
        if let Some(sel) = write
            .capture
            .as_ref()
            .and_then(|c| c.followup_select.as_ref())
        {
            statements.push(stmt("read-back of the written row", sel));
        }
    }
    if let Some(sel) = &lowered.ret_select {
        statements.push(stmt("return: re-select the declared shape", sel));
    }
    if let Some(readback) = &lowered.bulk_readback {
        statements.push(stmt(
            "return: re-select the written rows (keys spliced at /*BULK_KEYS*/)",
            &readback.sql,
        ));
    }
    Some(Explained {
        kind: CallableKind::Mutation,
        name: name.to_string(),
        statements,
    })
}

/// The write's own `-- …` header text without the comment markers.
fn write_label(header: &str, bulk: bool) -> String {
    let text = header
        .lines()
        .map(|l| l.trim_start_matches('-').trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let text = if text.is_empty() {
        "write".to_string()
    } else {
        text
    };
    match bulk {
        true => format!("{text} (template: one row per input at run time)"),
        false => text,
    }
}
