//! Value resolution: a `Value` to SQL (`resolve`), enum-column variant (`enum_lit`),
//! assignment RHS (`assign`). The stateless rendering leaves (`literals`, `raw`, `ops`)
//! stay flat at the dml root. Surface out: `Select` methods + `param_key`/`bref_name`.

mod assign;
mod enum_lit;
mod resolve;

pub(crate) use resolve::{bref_name, param_key};
