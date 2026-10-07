//! Strict v1 callback response decoding, including duplicate-field rejection.
use crate::guard::GuardVerdict;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u64,
    verdict: String,
    #[serde(default)]
    message: Option<String>,
}

pub(super) fn decode(bytes: &[u8]) -> Result<GuardVerdict, &'static str> {
    let response: Response = serde_json::from_slice(bytes).map_err(|_| "invalid response")?;
    if response.version != 1 {
        return Err("unsupported version");
    }
    match (response.verdict.as_str(), response.message) {
        ("allow", None) => {
            // An explicit null message is also forbidden on allow.
            let value: serde_json::Value =
                serde_json::from_slice(bytes).map_err(|_| "invalid response")?;
            if value.get("message").is_some() {
                return Err("unexpected message");
            }
            Ok(GuardVerdict::Allow)
        }
        ("deny", Some(message)) if !message.trim().is_empty() && message.len() <= 1024 => {
            Ok(GuardVerdict::deny(message))
        }
        _ => Err("invalid verdict"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_v1_responses_are_accepted() {
        assert_eq!(
            decode(br#"{"version":1,"verdict":"allow"}"#),
            Ok(GuardVerdict::Allow)
        );
        assert_eq!(
            decode(br#"{"version":1,"verdict":"deny","message":"No"}"#),
            Ok(GuardVerdict::deny("No"))
        );
        for body in [
            r#"{"version":1,"version":1,"verdict":"allow"}"#,
            r#"{"version":1,"verdict":"allow","verdict":"deny"}"#,
            r#"{"version":1,"verdict":"allow","message":null}"#,
            r#"{"version":1,"verdict":"allow","extra":true}"#,
            r#"{"version":1.0,"verdict":"allow"}"#,
            r#"{"version":2,"verdict":"allow"}"#,
            r#"{"version":1,"verdict":"deny","message":" "}"#,
            r#"{"version":1,"verdict":"deny"}"#,
            r#"{"version":1,"verdict":"allow"} {}"#,
        ] {
            assert!(decode(body.as_bytes()).is_err(), "{body}");
        }
    }
}
