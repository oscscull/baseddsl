/// Exact decimal value for generated clients. Emitted only when a client field
/// uses `Decimal`, so schemas without decimals need no BigDecimal dependency.
pub(crate) const DECIMAL: &str = r#"
/// An exact decimal backed by `bigdecimal::BigDecimal`. JSON always carries a
/// plain decimal string; no float conversion or scientific notation enters SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal(bigdecimal::BigDecimal);

impl std::str::FromStr for Decimal {
    type Err = bigdecimal::ParseBigDecimalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_plain_string())
    }
}

impl Serialize for Decimal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_plain_string())
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}
"#;
