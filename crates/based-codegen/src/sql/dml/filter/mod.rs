//! WHERE lowering: comparison rendering (`predicate`), the condition set (`collect`),
//! optional-input lowering (`optional`), named-filter inlining (`inline`). Surface out:
//! `Select::predicate` + `build_wheres`; the rest stays cluster-private.

mod collect;
mod inline;
mod optional;
mod predicate;

pub(crate) use collect::build_wheres;
