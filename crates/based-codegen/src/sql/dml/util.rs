//! Small shared helpers and grammar constants (column names, keys, spans).

use super::*;

/// A model's physical primary-key column for a JOIN / correlation `ON` — the `id`
/// column, or a `@key(field)` natural key. Falls back to `id` for a model with no
/// resolved key (a keyless model can't be a relation endpoint — sema flags that — so the
/// fallback only ever fires on an already-erroring schema, keeping the SQL well-formed).
pub(crate) fn pk_col(m: &RModel) -> String {
    m.pk_column().unwrap_or_else(|| "id".to_string())
}

/// The separator joining a nested to-one relation's field name to its projected
/// columns in a SELECT output alias (`buyer` + `name` → `buyer.name`). A `.` cannot
/// occur in a BSL identifier, so any output alias containing it is unambiguously a
/// nested projection — the runtime (`run.rs`) splits on it to reassemble the flat row
/// into a sub-object. One source of truth for the convention: codegen
/// emits it, the runtime reads it.
pub const NEST_SEP: char = '.';

/// The output-alias suffix marking a to-**many** nested array (`items { … }` → alias
/// `items[]`). The column's value is a JSON-array *string* (per-dialect JSON aggregation,
/// [`crate::Dialect::json_array_agg`]) of the nested sub-objects; the runtime (`run.rs`)
/// sees the `[]` suffix, parses the string into a real JSON array, and stores it under
/// the field name without the suffix. `[`/`]` cannot occur in a BSL identifier, so the
/// marker never collides with a projected field. One source of the convention: codegen
/// emits it, the runtime reads it. Composes with [`NEST_SEP`] — a to-many
/// inside a to-one nests as `parent.items[]`.
pub const ARRAY_MARK: &str = "[]";

/// The output-alias prefix for a keyset query's hidden cursor-basis columns. Each
/// sort key `k_i` is projected an extra time as `<k_i> AS __keyset_<i>` so the runtime
/// can read the last row's sort-key values to mint the next cursor, then strip these
/// columns from the response. The `__` prefix cannot begin a BSL identifier, so it can
/// never collide with a projected field. One source of the convention: codegen
/// emits it, the runtime (`run.rs`) reads + strips it.
pub const KEYSET_PREFIX: &str = "__keyset_";

/// Presence probe for a to-one nest whose row may be absent (a LEFT-JOINed edge:
/// an optional forward relation, or a to-one inverse). The child's `id` is projected
/// once more as `<field>.__present`; the runtime collapses the nested object to JSON
/// `null` when it is NULL (an all-null sub-object would otherwise be indistinguishable
/// from a matched row) and strips the probe. `__` cannot begin a BSL identifier.
pub const NEST_PRESENT: &str = "__present";

/// The bind name a `$name.field` reference reads from — the value the bound create's
/// row read-back captured for physical column `column`. One placeholder per
/// (binding, column); the run stage binds it from the re-selected row.
pub(crate) fn bref_name(binding: &str, column: &str) -> String {
    format!("bref_{binding}__{column}")
}

/// Physical column backing a scalar field (its `(column …)` override or its name).
pub(crate) fn column_of(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        _ => field.to_string(),
    }
}

/// Physical column backing any field: a scalar's column, or a forward relation's FK
/// (`<field>_id`). Falls back to the field name (inverse edge / unknown — the latter
/// sema already rejected). Used by the write side to map `field = $x` assignments.
pub(crate) fn physical_col(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
        _ => field.to_string(),
    }
}

/// A one-segment path, for the many call sites that resolve a single field name.
pub(crate) fn single(name: &str) -> Path {
    Path {
        segments: vec![Spanned {
            node: name.to_string(),
            span: NO_SPAN,
        }],
    }
}

const NO_SPAN: Span = Span {
    file: FileId(0),
    start: 0,
    end: 0,
};

pub(crate) fn push_prefix(prefix: &mut String, seg: &str) {
    if !prefix.is_empty() {
        prefix.push('.');
    }
    prefix.push_str(seg);
}

/// `$ctx.org` -> `ctx_org`; `$id` -> `id`. Placeholder-safe (dots removed).
pub(crate) fn param_key(pr: &ParamRef) -> String {
    let mut k = pr.name.node.clone();
    for seg in &pr.path {
        k.push('_');
        k.push_str(&seg.node);
    }
    k
}

/// Find the shape body for a return. `full` is per-model, so match on `from` too.
pub(crate) fn find_shape<'a>(decls: &'a [Decl], name: &str, model: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name && (name != "full" || s.from.node == model) => {
            Some(s)
        }
        _ => None,
    })
}
