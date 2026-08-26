//! The driver-agnostic bound value and JSON coercion.
//!
//! A wire request carries JSON; the driver wants typed scalars. `SqlValue` is the
//! neutral middle — one variant per storable family. Coercion is family-aware: the
//! signature says a param is an `int`, so a JSON string is rejected at the boundary
//! before any SQL runs. The typed text-riding variants (`Uuid`/`Timestamp`/`Date`/
//! `Decimal`) still carry their wire strings; only a concrete driver parses them into
//! its native parameter types (Postgres binds all parameters binary-typed, so the bind
//! site must know the value's primitive).

use based_ast::Primitive;

/// A bound argument that fills one placeholder. Neutral across drivers: the concrete
/// driver maps these onto its parameter binding. The typed variants carry the value's
/// wire *string* — parsing into a native driver type happens only inside driver impls.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    /// A canonical uuid string (also every engine `Id`).
    Uuid(String),
    Timestamp(String),
    Date(String),
    /// A time-of-day string (`HH:MM:SS[.ffffff]`).
    Time(String),
    /// An exact decimal string — never an `f64`.
    Decimal(String),
    /// A binary blob carried as its **base64** wire string — a concrete driver base64-decodes
    /// it to raw bytes at the bind, and base64-encodes a read value back to this form.
    Bytes(String),
    Json(serde_json::Value),
    /// A variable-length bind list for `col in $arr` (an array-typed param). It never reaches a
    /// driver: [`crate::scan::to_positional`] expands it into one positional placeholder per
    /// element (`(?, ?, …)`) at bind time, so each element binds as its own scalar.
    List(Vec<Self>),
}

impl SqlValue {
    /// How this value fills placeholders, for [`crate::scan::to_positional`]: a `List` expands
    /// to one bind per element — an **empty** list to a single `NULL`, so `col IN (:p)` renders
    /// `col IN (NULL)` (matches nothing) rather than the illegal `IN ()`; every scalar is one
    /// bind.
    pub(crate) fn expand(self) -> crate::scan::Bound<Self> {
        use crate::scan::Bound;
        match self {
            Self::List(items) if items.is_empty() => Bound::Many(vec![Self::Null]),
            Self::List(items) => Bound::Many(items),
            other => Bound::One(other),
        }
    }
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
    Text,
    /// `uuid` and `Id` — a uuid string on the wire, a native uuid at a typed bind.
    Uuid,
    Timestamp,
    Date,
    /// `time` — a time-of-day string on the wire.
    Time,
    /// `decimal` — its exact string on the wire, never an `f64`.
    Decimal,
    /// `bytes` — a base64 string on the wire, decoded to raw bytes at a typed bind.
    Bytes,
    /// `json` — any JSON value passes through unchanged.
    Json,
    /// Untyped: coerce by the JSON value's own shape (a string stays a plain text
    /// bind — a raw-SQL query comparing one against a typed column writes the cast).
    Any,
}

impl Family {
    /// The family a primitive coerces to.
    pub fn of(prim: Primitive) -> Self {
        match prim {
            // `serial` is a DB-generated 64-bit integer id — it binds and reads back as
            // an `int` (a JSON number), never a uuid string.
            Primitive::Int | Primitive::Serial => Self::Int,
            Primitive::Float => Self::Float,
            Primitive::Bool => Self::Bool,
            Primitive::Json => Self::Json,
            Primitive::Decimal { .. } => Self::Decimal,
            Primitive::Bytes => Self::Bytes,
            Primitive::Text => Self::Text,
            Primitive::Timestamp => Self::Timestamp,
            Primitive::Date => Self::Date,
            Primitive::Time => Self::Time,
            // `ulid` is an app-minted sortable string id — same wire family as `uuid`.
            Primitive::Uuid | Primitive::Id | Primitive::Ulid => Self::Uuid,
        }
    }

    /// A human name for the family, for a boundary error message.
    pub fn label(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Text => "text",
            Self::Uuid => "uuid",
            Self::Timestamp => "timestamp",
            Self::Date => "date",
            Self::Time => "time",
            Self::Decimal => "decimal",
            Self::Bytes => "bytes",
            Self::Json => "json",
            Self::Any => "value",
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
                .filter(|u| i64::try_from(*u).is_ok())
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
        Family::Uuid => match v {
            J::String(s) => Ok(SqlValue::Uuid(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        Family::Timestamp => match v {
            J::String(s) => Ok(SqlValue::Timestamp(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        Family::Date => match v {
            J::String(s) => Ok(SqlValue::Date(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        Family::Time => match v {
            J::String(s) => Ok(SqlValue::Time(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        Family::Decimal => match v {
            J::String(s) => Ok(SqlValue::Decimal(s.clone())),
            _ => mismatch(json_kind(v)),
        },
        // A `bytes` value crosses the wire as a base64 string; the concrete driver decodes
        // it to raw bytes at the bind (validated there, uniformly across drivers).
        Family::Bytes => match v {
            J::String(s) => Ok(SqlValue::Bytes(s.clone())),
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
        J::Number(n) if n.is_u64() && i64::try_from(n.as_u64().unwrap()).is_ok() => {
            SqlValue::Int(n.as_u64().unwrap() as i64)
        }
        J::Number(n) => SqlValue::Float(n.as_f64().unwrap_or(0.0)),
        J::String(s) => SqlValue::Text(s.clone()),
        J::Array(_) | J::Object(_) => SqlValue::Json(v.clone()),
    }
}

/// base64 (standard alphabet, padded) between a `bytes` value's wire string and its raw
/// bytes — the concrete drivers decode at a bind and encode a read `BYTEA`/`BLOB` value
/// back. Kept here so all three drivers share one codec.
#[cfg(any(feature = "mariadb", feature = "postgres", feature = "sqlite"))]
pub(crate) fn b64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s.trim())
}

#[cfg(any(feature = "mariadb", feature = "postgres", feature = "sqlite"))]
pub(crate) fn b64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
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
