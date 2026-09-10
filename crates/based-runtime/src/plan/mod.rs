//! Planning a query request: validate → thread `$ctx` → bind → pick the envelope.
//!
//! This is the runtime's core. It reads the signature (AST `Query` for the params,
//! `RQuery` for the inferred verb / cardinality / pagination and the `$ctx`
//! requirement bag) and the lowered SQL, and produces an executable [`QueryPlan`]:
//! positional statements + the [`Envelope`] the rows are shaped into.
//!
//! Binding uses the fact that codegen's placeholder names are unambiguous given the
//! schema: a declared param renders `:<param>`, a context field `:ctx_<field>`, offset
//! pagination `:offset`. So the runtime assembles one value environment from the
//! validated inputs and lets [`crate::scan::to_positional`] pull from it in SQL order.

use based_ast::{
    Assign, AssignRhs, BaseType, Decl, DefaultVal, Literal, Mutation, Param, Path, Predicate,
    Primitive, Query, Value, WriteStmt,
};
use based_ast::{NamedFilter, Verb};
use based_sema::{CheckedSchema, CtxField, CtxReq, MemberKind, RModel};

use crate::id::IdGen;
use crate::load::Compiled;
use crate::scan::to_positional;
use crate::value::{coerce, CoerceError, Family, SqlValue};

mod bind;
mod bulk;
mod ir;
mod lookup;
mod mutation;
mod paginate;
mod param_use;
mod query;

pub(crate) use bind::*;
pub(crate) use bulk::*;
pub use ir::*;
pub(crate) use lookup::*;
pub use mutation::*;
pub(crate) use paginate::*;
pub(crate) use param_use::*;
pub use query::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The fingerprint is stable for the same payload, differs when args or `$ctx` change,
    /// and is invariant to the idempotency key (the store scopes by key already — the
    /// fingerprint's only job is to detect a payload change under a reused key).
    #[test]
    fn fingerprint_tracks_the_payload_not_the_key() {
        let base = Request::new("m", json!({ "a": 1, "b": "x" }), json!({ "org": "o-1" }));

        // Same payload → same fingerprint, whatever the key.
        let same = base
            .clone()
            .with_idempotency_key(Some("different-key".into()));
        assert_eq!(base.fingerprint(), same.fingerprint());

        // Key order in the JSON object does not matter (BTreeMap-backed, sorted).
        let reordered = Request::new("m", json!({ "b": "x", "a": 1 }), json!({ "org": "o-1" }));
        assert_eq!(base.fingerprint(), reordered.fingerprint());

        // A different arg value → a different fingerprint.
        let arg_changed = Request::new("m", json!({ "a": 2, "b": "x" }), json!({ "org": "o-1" }));
        assert_ne!(base.fingerprint(), arg_changed.fingerprint());

        // A different `$ctx` → a different fingerprint.
        let ctx_changed = Request::new("m", json!({ "a": 1, "b": "x" }), json!({ "org": "o-2" }));
        assert_ne!(base.fingerprint(), ctx_changed.fingerprint());

        // Moving a field between args and ctx changes the hash (the separator prevents an
        // ambiguous concatenation collapsing the two maps).
        let a = Request::new("m", json!({ "x": 1 }), json!({}));
        let b = Request::new("m", json!({}), json!({ "x": 1 }));
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
