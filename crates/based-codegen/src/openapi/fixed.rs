//! The fixed shared component schemas: the error envelope, the `{ id }` mutation
//! result, and the empty `-> ok` acknowledgement.

use super::*;

/// The error envelope schema: `{ "error": { code, message } }` — exactly what
/// `serve::dispatch` returns for a boundary (`4xx`) or database (`503`) failure.
pub(crate) fn error_schema() -> Value {
    json!({
        "type": "object",
        "required": ["error"],
        "properties": {
            "error": {
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": { "type": "string" },
                    "message": { "type": "string" }
                }
            }
        }
    })
}

/// The `{ id }` schema a mutation with no declared return shape responds with.
pub(crate) fn mutation_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id"],
        "properties": { "id": uuid_schema() },
        "description": "A write with no declared-shape re-select responds with the id \
                        of the affected row."
    })
}

/// The `-> ok` acknowledgement: an empty object. Registered only when the schema
/// declares an ack mutation.
pub(crate) fn ack_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
        "description": "The empty acknowledgement of a `-> ok` mutation: the delete \
                        ran; a real DELETE leaves no row to return."
    })
}
