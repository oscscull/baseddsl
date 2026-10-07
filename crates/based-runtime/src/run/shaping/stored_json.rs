//! Decode JSON at schema-declared JSON leaves or aggregate-array positions.

pub(super) fn parse_stored_json(
    text: &str,
    dialect: based_codegen::Dialect,
) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    // MariaDB JSON is binary-collated LONGTEXT. The MySQL wire reports it as a
    // blob, which the driver must base64-encode to preserve actual bytes columns.
    // Only schema-identified JSON positions unwrap that representation here.
    if !matches!(
        dialect,
        based_codegen::Dialect::MariaDb | based_codegen::Dialect::MySql
    ) {
        return None;
    }
    #[cfg(feature = "mariadb")]
    {
        let bytes = crate::value::b64_decode(text).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    #[cfg(not(feature = "mariadb"))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use based_codegen::Dialect;

    #[test]
    fn postgres_json_strings_parse_without_base64_unwrapping() {
        for text in ["true", "123", "eyJzZWNyZXQiOnRydWV9"] {
            let stored = serde_json::to_string(text).unwrap();
            assert_eq!(
                parse_stored_json(&stored, Dialect::Postgres),
                Some(serde_json::json!(text))
            );
        }
        assert!(parse_stored_json("eyJzZWNyZXQiOnRydWV9", Dialect::Postgres).is_none());
    }

    #[cfg(feature = "mariadb")]
    #[test]
    fn blob_json_decoding_is_limited_to_mysql_dialects() {
        let encoded = crate::value::b64_encode(br#"{"value": [null, true]}"#);
        assert_eq!(
            parse_stored_json(&encoded, Dialect::MariaDb),
            Some(serde_json::json!({"value":[null,true]}))
        );
        assert!(parse_stored_json(&encoded, Dialect::Sqlite).is_none());
        assert_eq!(
            parse_stored_json(r#""true""#, Dialect::MariaDb),
            Some(serde_json::json!("true"))
        );
    }
}
