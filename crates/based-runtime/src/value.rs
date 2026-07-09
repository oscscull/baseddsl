//! The driver-agnostic bound value and JSON coercion.
//!
//! A wire request carries JSON; the driver wants typed scalars. `SqlValue` is the
//! neutral middle — one variant per storable family. Coercion is family-aware: the
//! signature says a param is an `int`, so a JSON string is rejected at the boundary
//! before any SQL runs. Families are coarse on purpose, matching sema's `=`-operand
//! families: `uuid`/`timestamp`/`date`/`Id` all ride as text, since on the wire they are
//! strings.

use based_ast::Primitive;

/// A bound argument that fills one placeholder. Neutral across drivers: the concrete
/// driver maps these onto its parameter binding.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    Json(serde_json::Value),
}

/// The coercion target for one input: the family a JSON value must fit, plus
/// whether `null` is allowed. `Any` accepts a value shaped however the JSON is
/// (used for an untyped param whose column type the runtime does not re-derive —
/// the typed client already sends the right shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Int,
    Float,
    Bool,
    /// `text` and everything wire-encoded as a string: `uuid`/`timestamp`/`date`/`Id`.
    Text,
    /// `json` — any JSON value passes through unchanged.
    Json,
    /// Untyped: coerce by the JSON value's own shape.
    Any,
}

impl Family {
    /// The family a primitive coerces to. The text-riding types collapse to `Text`.
    pub fn of(prim: Primitive) -> Family {
        match prim {
            Primitive::Int => Family::Int,
            Primitive::Bool => Family::Bool,
            Primitive::Json => Family::Json,
            Primitive::Text
            | Primitive::Timestamp
            | Primitive::Date
            | Primitive::Uuid
            | Primitive::Id => Family::Text,
        }
    }

    /// A human name for the family, for a boundary error message.
    pub fn label(self) -> &'static str {
        match self {
            Family::Int => "int",
            Family::Float => "float",
            Family::Bool => "bool",
            Family::Text => "text",
            Family::Json => "json",
            Family::Any => "value",
        }
    }
}

/// Why a value did not fit its expected family — carried up into a `PlanError`.
#[derive(Debug, Clone, PartialEq)]
pub struct CoerceError {
    pub expected: Family,
    pub got: String,
}

/// Coerce one JSON value into a `SqlValue` of the expected family. `optional`
/// governs whether JSON `null` is accepted (→ `SqlValue::Null`); a required field
/// with a `null` is an error, same as a missing one.
pub fn coerce(
    v: &serde_json::Value,
    family: Family,
    optional: bool,
) -> Result<SqlValue, CoerceError> {
    use serde_json::Value as J;
    if v.is_null() {
        if optional {
            return Ok(SqlValue::Null);
        }
        return Err(CoerceError {
            expected: family,
            got: "null".to_string(),
        });
    }
    let mismatch = |got: &str| {
        Err(CoerceError {
            expected: family,
            got: got.to_string(),
        })
    };
    match family {
        Family::Int => match v {
            // Accept an integer-valued number; reject a fractional one (silent
            // truncation would violate "nothing consequential by omission").
            J::Number(n) if n.is_i64() => Ok(SqlValue::Int(n.as_i64().unwrap())),
            J::Number(n) if n.is_u64() => n
                .as_u64()
                .filter(|u| *u <= i64::MAX as u64)
                .map(|u| SqlValue::Int(u as i64))
                .ok_or_else(|| CoerceError {
                    expected: family,
                    got: "integer out of range".to_string(),
                }),
            J::Number(_) => mismatch("fractional number"),
            _ => mismatch(json_kind(v)),
        },
        Family::Float => match v {
            J::Number(n) => Ok(SqlValue::Float(n.as_f64().unwrap_or(0.0))),
            _ => mismatch(json_kind(v)),
        },
        Family::Bool => match v {
            J::Bool(b) => Ok(SqlValue::Bool(*b)),
            _ => mismatch(json_kind(v)),
        },
        Family::Text => match v {
            J::String(s) => Ok(SqlValue::Text(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        Family::Json => Ok(SqlValue::Json(v.clone())),
        Family::Any => Ok(by_shape(v)),
    }
}

/// Coerce by the JSON value's own shape (an untyped param). Never fails.
fn by_shape(v: &serde_json::Value) -> SqlValue {
    use serde_json::Value as J;
    match v {
        J::Null => SqlValue::Null,
        J::Bool(b) => SqlValue::Bool(*b),
        J::Number(n) if n.is_i64() => SqlValue::Int(n.as_i64().unwrap()),
        J::Number(n) if n.is_u64() && n.as_u64().unwrap() <= i64::MAX as u64 => {
            SqlValue::Int(n.as_u64().unwrap() as i64)
        }
        J::Number(n) => SqlValue::Float(n.as_f64().unwrap_or(0.0)),
        J::String(s) => SqlValue::Text(s.clone()),
        J::Array(_) | J::Object(_) => SqlValue::Json(v.clone()),
    }
}

/// Human name of a JSON value's kind, for the "expected X, got Y" error.
fn json_kind(v: &serde_json::Value) -> &'static str {
    use serde_json::Value as J;
    match v {
        J::Null => "null",
        J::Bool(_) => "boolean",
        J::Number(_) => "number",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    }
}
