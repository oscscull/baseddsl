//! Strict decoding of the v1 callback verdict, including duplicate/unknown fields.
use crate::GuardVerdict;
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
enum Response {
    Allow(Allow),
    Deny(Deny),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Allow {
    version: u64,
    verdict: AllowTag,
}

#[derive(Deserialize)]
enum AllowTag {
    #[serde(rename = "allow")]
    Allow,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deny {
    version: u64,
    verdict: DenyTag,
    message: String,
}

#[derive(Deserialize)]
enum DenyTag {
    #[serde(rename = "deny")]
    Deny,
}

pub(super) fn decode(bytes: &[u8]) -> Result<GuardVerdict, &'static str> {
    match serde_json::from_slice::<Response>(bytes).map_err(|_| "protocol")? {
        Response::Allow(Allow {
            version: 1,
            verdict: AllowTag::Allow,
        }) => Ok(GuardVerdict::Allow),
        Response::Deny(Deny {
            version: 1,
            verdict: DenyTag::Deny,
            message,
        }) if !message.trim().is_empty() && message.len() <= 1024 => {
            Ok(GuardVerdict::deny(message))
        }
        _ => Err("protocol"),
    }
}

pub(super) fn validate_headers(response: &reqwest::Response) -> Result<(), &'static str> {
    if response.status() != reqwest::StatusCode::OK {
        return Err("HTTP status");
    }
    let headers = response.headers();
    let json = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    let identity = headers
        .get(CONTENT_ENCODING)
        .is_none_or(|v| v == "identity");
    if !json
        || !identity
        || headers.get_all(CONTENT_TYPE).iter().count() != 1
        || headers.get_all(CONTENT_ENCODING).iter().count() > 1
    {
        return Err("response headers");
    }
    if response
        .content_length()
        .is_some_and(|n| n > super::MAX_RESPONSE as u64)
    {
        return Err("response size");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_or_extended_verdicts() {
        for body in [
            r#"{"version":1,"version":1,"verdict":"allow"}"#,
            r#"{"version":1,"verdict":"allow","verdict":"deny"}"#,
            r#"{"version":1,"verdict":"allow","message":null}"#,
            r#"{"version":1,"verdict":"allow","extra":true}"#,
            r#"{"version":1.0,"verdict":"allow"}"#,
            r#"{"version":2,"verdict":"allow"}"#,
            r#"{"version":1,"verdict":"deny","message":" "}"#,
            r#"{"version":1,"verdict":"deny","message":"no","message":"yes"}"#,
            r#"{"version":1,"verdict":"allow"} {}"#,
        ] {
            assert!(decode(body.as_bytes()).is_err(), "{body}");
        }
        let oversized =
            serde_json::json!({"version":1,"verdict":"deny","message":"x".repeat(1025)});
        assert!(decode(&serde_json::to_vec(&oversized).unwrap()).is_err());
    }
}
