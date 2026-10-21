//! WHERE lowering: comparison rendering (`predicate`), the condition set (`collect`),
//! optional-input lowering (`optional`), named-filter inlining (`inline`), to-many `EXISTS`
//! lowering (`relation_any`, `to_many_leaf`), nested-row rooting (`predicate_at`). Surface out:
//! `Select::predicate` + `build_wheres`; the rest stays cluster-private.

mod collect;
mod inline;
mod optional;
mod predicate;
mod predicate_at;
mod relation_any;
mod to_many_leaf;

pub(crate) use collect::build_wheres;
