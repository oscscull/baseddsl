//! Opaque keyset cursor encode/decode (pagination.md).
//!
//! A keyset page returns a cursor the caller passes back for the next page. The cursor
//! is **opaque** (the caller never assembles keyset mechanics) and **validated** (a
//! corrupt/tampered cursor is rejected, not fed to the query). It carries the previous
//! page's last-row sort-key values — exactly the `ORDER BY` basis codegen emitted as
//! hidden `__keyset_<i>` columns — which the runtime binds into the `:keyset_<i>`
//! placeholders of the cursor comparison.
//!
//! Wire form: `<checksum-hex>.<payload-hex>`, where the payload is the JSON array of
//! sort-key values and the checksum is an FNV-1a hash of it. The checksum catches
//! corruption/truncation and cheap tampering; it is **not** a cryptographic signature
//! (that needs a server secret — deferred). The real safety property — no predicate
//! injection — holds regardless: cursor values only ever fill bound parameters, never
//! concatenate into SQL, so even a forged cursor can shift *which* rows are returned
//! (values the caller could read off the results anyway) but never inject SQL.

use crate::value::{coerce, Family, SqlValue};

/// Why an incoming cursor could not be decoded — always a client error (a malformed or
/// tampered cursor), surfaced as a `PlanError::BadCursor` (→ 400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorError(pub String);

/// Encode the last row's sort-key values into an opaque cursor string.
pub fn encode(values: &[serde_json::Value]) -> String {
    let payload = serde_json::Value::Array(values.to_vec()).to_string();
    let sum = fnv1a(payload.as_bytes());
    format!("{sum:016x}.{}", hex(payload.as_bytes()))
}

/// Decode a cursor into `n` bound values, validating the checksum, structure, and
/// arity. Any deviation is a `CursorError` (the caller sent a bad cursor).
pub fn decode(s: &str, n: usize) -> Result<Vec<SqlValue>, CursorError> {
    let (sum_hex, payload_hex) = s
        .split_once('.')
        .ok_or_else(|| CursorError("malformed cursor".into()))?;
    let sum = u64::from_str_radix(sum_hex, 16)
        .map_err(|_| CursorError("malformed cursor checksum".into()))?;
    let payload =
        unhex(payload_hex).ok_or_else(|| CursorError("malformed cursor payload".into()))?;
    if fnv1a(&payload) != sum {
        return Err(CursorError("cursor failed integrity check".into()));
    }
    let json: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| CursorError("malformed cursor payload".into()))?;
    let arr = json
        .as_array()
        .ok_or_else(|| CursorError("malformed cursor payload".into()))?;
    if arr.len() != n {
        return Err(CursorError("cursor arity mismatch".into()));
    }
    // `Family::Any` coerces each value by its own JSON shape — the sort keys ride back
    // exactly as they came out of the DB (text/number/bool), so the comparison binds
    // the same value the row carried.
    Ok(arr
        .iter()
        .map(|v| coerce(v, Family::Any, true).expect("Any coercion is infallible"))
        .collect())
}

/// FNV-1a 64-bit — the same lightweight non-cryptographic hash the idempotency
/// fingerprint uses (plan.rs); here it is a tamper/corruption checksum, not a signature.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_values() {
        let vals = vec![json!("2024-01-01T00:00:00Z"), json!("abc-123")];
        let cur = encode(&vals);
        let got = decode(&cur, 2).unwrap();
        assert_eq!(
            got,
            vec![
                SqlValue::Text("2024-01-01T00:00:00Z".into()),
                SqlValue::Text("abc-123".into()),
            ]
        );
    }

    #[test]
    fn round_trips_mixed_families() {
        let vals = vec![json!(42), json!(true), json!("x")];
        let cur = encode(&vals);
        assert_eq!(
            decode(&cur, 3).unwrap(),
            vec![
                SqlValue::Int(42),
                SqlValue::Bool(true),
                SqlValue::Text("x".into())
            ]
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let cur = encode(&[json!("a")]);
        // Flip the last payload nibble; the checksum no longer matches.
        let mut bytes: Vec<char> = cur.chars().collect();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == '0' { '1' } else { '0' };
        let tampered: String = bytes.into_iter().collect();
        assert!(decode(&tampered, 1).is_err());
    }

    #[test]
    fn rejects_wrong_arity() {
        let cur = encode(&[json!("a"), json!("b")]);
        assert!(decode(&cur, 3).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("not-a-cursor", 1).is_err());
        assert!(decode("deadbeef.zzzz", 1).is_err());
    }
}
