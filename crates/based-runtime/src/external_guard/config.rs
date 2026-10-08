//! Callback configuration validation and redacted startup errors.
/// Configuration failures contain no endpoint or secret values.
#[derive(Debug)]
pub struct ConfigError(pub(super) &'static str);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ConfigError {}

fn validate_endpoint(value: &str, allow_http: bool) -> Result<reqwest::Url, ConfigError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| ConfigError("invalid guard URL (value redacted)"))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ConfigError(
            "guard URLs must not contain credentials or fragments",
        ));
    }
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    let host = match authority.strip_prefix("[::1]") {
        Some("") => "[::1]",
        Some(suffix) if suffix.starts_with(':') => "[::1]",
        _ => authority.split(':').next().unwrap_or_default(),
    };
    let loopback = matches!(host, "127.0.0.1" | "[::1]") || host.eq_ignore_ascii_case("localhost");
    if url.host_str().is_none()
        || !(url.scheme() == "https" || (url.scheme() == "http" && allow_http && loopback))
    {
        return Err(ConfigError(
            "guard URL requires verified HTTPS; development HTTP permits only loopback",
        ));
    }
    Ok(url)
}

pub(super) fn validate(
    name: &str,
    endpoint: &str,
    secret: &str,
    deadline: std::time::Duration,
    allow_loopback_http: bool,
) -> Result<(reqwest::Url, reqwest::header::HeaderValue), ConfigError> {
    let endpoint = validate_endpoint(endpoint, allow_loopback_http)?;
    if name.trim().is_empty() || secret.trim().is_empty() {
        return Err(ConfigError("guard name and bearer secret must be nonempty"));
    }
    if deadline.is_zero() || deadline > std::time::Duration::from_secs(30) {
        return Err(ConfigError(
            "guard deadline must be positive and at most 30 seconds",
        ));
    }
    let mut authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {secret}"))
        .map_err(|_| ConfigError("invalid guard bearer secret (value redacted)"))?;
    authorization.set_sensitive(true);
    Ok((endpoint, authorization))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rejects_invalid_trust_configuration_without_exposing_values() {
        for endpoint in [
            "bogus-secret",
            "http://example.com",
            "http://127.1",
            "http://2130706433",
            "ftp://localhost",
            "https://user:secret@localhost",
            "https://localhost/#secret",
        ] {
            let err =
                validate("guard", endpoint, "secret", Duration::from_secs(2), true).unwrap_err();
            assert!(!err.to_string().contains("secret"), "{err}");
        }
        assert!(validate_endpoint("http://localhost", false).is_err());
        for endpoint in [
            "http://127.0.0.1:99",
            "http://[::1]:99",
            "http://localhost:99",
            "https://example.com",
        ] {
            assert!(validate_endpoint(endpoint, true).is_ok());
        }
        for deadline in [Duration::ZERO, Duration::from_secs(31)] {
            assert!(validate("guard", "https://localhost", "secret", deadline, false).is_err());
        }
        for secret in ["", " ", "secret\nheader"] {
            assert!(validate(
                "guard",
                "https://localhost",
                secret,
                Duration::from_secs(2),
                false
            )
            .is_err());
        }
    }
}
