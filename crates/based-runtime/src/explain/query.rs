use super::{label::param_label, positional::positional, CallableKind, Explained};
use crate::load::Compiled;
use crate::plan::find_query;

/// The main SELECT, then the live-row count of a `with count` page.
pub(super) fn explain(compiled: &Compiled, name: &str) -> Option<Explained> {
    let lowered = compiled.queries.get(name)?;
    let params = find_query(&compiled.decls, name).map_or(&[][..], |q| &q.params[..]);
    let stmt = |label: &str, sql: &str| {
        positional(label, sql, compiled.dialect, |p| param_label(params, p))
    };
    let mut statements = vec![stmt("select", &lowered.sql)];
    if let Some(count) = &lowered.count_sql {
        statements.push(stmt("count", count));
    }
    Some(Explained {
        kind: CallableKind::Query,
        name: name.to_string(),
        statements,
    })
}
