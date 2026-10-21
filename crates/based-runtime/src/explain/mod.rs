//! The statements a callable executes, in run order, as the driver receives them:
//! positional SQL plus which value fills each placeholder. Read off the same lowering and
//! placeholder scan the engine runs, so it cannot drift from execution.

mod label;
mod mutation;
mod positional;
mod query;

use crate::load::Compiled;

/// Whether a callable is a read or a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableKind {
    Query,
    Mutation,
}

/// One callable's statements, in execution order.
#[derive(Debug, Clone)]
pub struct Explained {
    pub kind: CallableKind,
    pub name: String,
    pub statements: Vec<ExplainedStmt>,
}

/// One executed statement: what it is, its positional SQL, and the value each
/// placeholder binds, in bind order (`params[0]` is placeholder 1).
#[derive(Debug, Clone)]
pub struct ExplainedStmt {
    pub label: String,
    pub sql: String,
    pub params: Vec<String>,
}

/// Every callable named `name` (a query and a mutation may share one).
pub fn explain(compiled: &Compiled, name: &str) -> Vec<Explained> {
    query::explain(compiled, name)
        .into_iter()
        .chain(mutation::explain(compiled, name))
        .collect()
}

/// Every callable in declaration order, queries first.
pub fn explain_all(compiled: &Compiled) -> Vec<Explained> {
    let queries = compiled.schema.queries.iter();
    let mutations = compiled.schema.mutations.iter();
    queries
        .filter_map(|q| query::explain(compiled, &q.name))
        .chain(mutations.filter_map(|m| mutation::explain(compiled, &m.name)))
        .collect()
}

/// Every callable name in declaration order, queries first.
pub fn callable_names(compiled: &Compiled) -> Vec<&str> {
    let queries = compiled.schema.queries.iter().map(|q| q.name.as_str());
    let mutations = compiled.schema.mutations.iter().map(|m| m.name.as_str());
    queries.chain(mutations).collect()
}
