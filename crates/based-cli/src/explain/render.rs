//! Text rendering of explained callables.
use based_codegen::Dialect;
use based_runtime::explain::{CallableKind, Explained, ExplainedStmt};

/// One callable: a header, then each statement in execution order.
pub fn callable(explained: &Explained, dialect: Dialect) -> String {
    let kind = match explained.kind {
        CallableKind::Query => "query",
        CallableKind::Mutation => "mutation",
    };
    let mut out = format!("-- {kind} {} ({})\n", explained.name, dialect.name());
    for (i, stmt) in explained.statements.iter().enumerate() {
        out.push('\n');
        out.push_str(&statement(i + 1, stmt));
    }
    out
}

fn statement(step: usize, stmt: &ExplainedStmt) -> String {
    let mut out = format!("-- [{step}] {}\n{}\n", stmt.label, stmt.sql);
    if stmt.params.is_empty() {
        out.push_str("-- binds: none\n");
        return out;
    }
    out.push_str("-- binds:\n");
    for (i, param) in stmt.params.iter().enumerate() {
        out.push_str(&format!("--   {}  {param}\n", i + 1));
    }
    out
}
