use super::*;

/// Reject a `limit ($p)` value below 1 or above its `max`. A literal limit was
/// checked at compile time.
pub(crate) fn check_limit(ast: &Query, env: &Env) -> Result<(), PlanError> {
    use based_ast::{Clause, LimitValue, QueryBody};
    let clauses: &[Clause] = match &ast.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    let Some((name, max)) = clauses.iter().find_map(|c| match c {
        Clause::Limit(l) => match &l.value {
            LimitValue::Param(p) => Some((&p.name.node, l.max)),
            LimitValue::Lit(_) => None,
        },
        _ => None,
    }) else {
        return Ok(());
    };
    let Some(SqlValue::Int(got)) = env.values.get(name) else {
        return Ok(());
    };
    let over = max.is_some_and(|m| u64::try_from(*got).is_ok_and(|g| g > m));
    if *got < 1 || over {
        return Err(PlanError::BadLimit {
            name: name.clone(),
            got: *got,
            max,
        });
    }
    Ok(())
}
