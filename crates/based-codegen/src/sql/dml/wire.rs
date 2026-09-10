//! The column-naming contract shared with the runtime (how nested JSON is encoded in a flat row).

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
