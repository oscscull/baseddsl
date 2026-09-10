use super::*;

/// Whether the query paginates by offset (its lowered SQL carries `:offset`).
pub(crate) fn offset_paginated(ast: &Query) -> bool {
    use based_ast::{Clause, QueryBody};
    let clauses: &[Clause] = match &ast.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    clauses
        .iter()
        .any(|c| matches!(c, Clause::Page(p) if p.offset))
}

/// The `page (...)` size of a paginated query (`None` when it does not paginate).
pub(crate) fn page_size(ast: &Query) -> Option<u64> {
    use based_ast::{Clause, QueryBody};
    let clauses: &[Clause] = match &ast.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    clauses.iter().find_map(|c| match c {
        Clause::Page(p) => Some(p.size),
        _ => None,
    })
}
