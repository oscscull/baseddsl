use based_ast::Primitive;
use based_sema::{EnumValue, REnum};

/// The value a builder fills into a required column nobody set. `n` is one number per
/// fixture from a per-process sequence that starts at a random offset, so unique columns
/// never collide, even across processes sharing a database.
#[derive(Debug, Clone, PartialEq)]
pub enum Placeholder {
    /// `"<field>-<n>"`.
    Text,
    /// `n`.
    Int,
    /// `n` as a float.
    Float,
    /// `n`, cut to the integer digits the column holds, as an exact decimal string.
    Decimal { whole_digits: u32 },
    /// `false`.
    Bool,
    /// The enum's first variant (its wire value).
    Enum(EnumValue),
    /// 2000-01-01T00:00:00Z plus `n` seconds, within about 31 years.
    Timestamp,
    /// 2000-01-01 plus `n` days, within 100 years.
    Date,
    /// A time of day from `n` seconds.
    Time,
    /// `{}`.
    Json,
    /// Empty bytes.
    Bytes,
    /// A uuid spelling `n` in its first and last groups.
    Uuid,
    /// A ulid whose digits are `n`.
    Ulid,
    /// An empty array column.
    Empty,
}

impl Placeholder {
    /// Whether the value depends on the fixture's number.
    pub fn uses_n(&self) -> bool {
        !matches!(
            self,
            Self::Bool | Self::Enum(_) | Self::Json | Self::Bytes | Self::Empty
        )
    }

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
            Primitive::Decimal { precision, scale } => Self::Decimal {
                whole_digits: precision.saturating_sub(scale).min(18),
            },
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
