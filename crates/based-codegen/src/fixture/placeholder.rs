use based_ast::Primitive;
use based_sema::{EnumValue, REnum};

/// The value a builder fills into a required column nobody set. `n` is one number per
/// fixture, drawn from a process-wide counter, so unique columns never collide.
#[derive(Debug, Clone, PartialEq)]
pub enum Placeholder {
    /// `"<field>-<n>"`.
    Text,
    /// `n`.
    Int,
    /// `n` as a float.
    Float,
    /// `n` as an exact decimal string.
    Decimal,
    /// `false`.
    Bool,
    /// The enum's first variant (its wire value).
    Enum(EnumValue),
    /// 2000-01-01T00:00:00Z plus `n` seconds.
    Timestamp,
    /// 2000-01-01 plus `n` days.
    Date,
    /// Midnight plus `n` seconds.
    Time,
    /// `{}`.
    Json,
    /// Empty bytes.
    Bytes,
    /// A uuid whose last group is `n`.
    Uuid,
    /// A ulid whose digits are `n`.
    Ulid,
    /// An empty array column.
    Empty,
}

impl Placeholder {
    pub(super) fn of(ty: Primitive, enum_decl: Option<&REnum>, many: bool) -> Self {
        if many {
            return Self::Empty;
        }
        if let Some(first) = enum_decl.and_then(|e| e.variants.first()) {
            return Self::Enum(first.value.clone());
        }
        match ty {
            Primitive::Text => Self::Text,
            Primitive::Int | Primitive::Serial => Self::Int,
            Primitive::Float => Self::Float,
            Primitive::Decimal { .. } => Self::Decimal,
            Primitive::Bool => Self::Bool,
            Primitive::Timestamp => Self::Timestamp,
            Primitive::Date => Self::Date,
            Primitive::Time => Self::Time,
            Primitive::Json => Self::Json,
            Primitive::Bytes => Self::Bytes,
            Primitive::Uuid | Primitive::Id => Self::Uuid,
            Primitive::Ulid => Self::Ulid,
        }
    }
}
