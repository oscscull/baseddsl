//! Authenticated, bounded HTTP preflight checks over the shared guard registry.
mod protocol;

use crate::guard::{GuardRequest, GuardVerdict, Guards};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE};
use std::time::Duration;

const MAX_REQUEST: usize = 256 * 1024;
const MAX_RESPONSE: usize = 16 * 1024;

/// A validated operator-owned callback. Debug output deliberately omits credentials and URLs.
#[derive(Clone)]
pub struct HttpGuard {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    authorization: HeaderValue,
    deadline: Duration,
}

impl HttpGuard {
    /// HTTPS is required unless the operator explicitly permits HTTP on literal loopback hosts.
    pub fn new(
        endpoint: &str,
        secret: &str,
        deadline: Duration,
        allow_loopback_http: bool,
    ) -> Result<Self, &'static str> {
        let endpoint = reqwest::Url::parse(endpoint).map_err(|_| "invalid callback URL")?;
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err("callback URL must have no credentials or fragment");
        }
        let loopback = matches!(
            endpoint.host_str(),
            Some("127.0.0.1" | "[::1]" | "localhost")
        );
        if endpoint.scheme() != "https"
            && !(endpoint.scheme() == "http" && loopback && allow_loopback_http)
        {
            return Err("callback requires HTTPS (explicit development HTTP allows loopback only)");
        }
        if endpoint.host_str().is_none() || deadline.is_zero() || deadline > Duration::from_secs(30)
        {
            return Err("callback requires a host and deadline of 1–30000 milliseconds");
        }
        if secret.trim().is_empty() {
            return Err("callback bearer secret is empty");
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|_| "invalid callback bearer secret")?;
        authorization.set_sensitive(true);
        let client = client_builder()?
            .build()
            .map_err(|_| "cannot build callback client")?;
        Ok(Self {
            client,
            endpoint,
            authorization,
            deadline,
        })
    }

    /// Trust an operator-owned PEM CA in addition to the bundled Mozilla roots.
    pub fn with_ca_pem(mut self, pem: &[u8]) -> Result<Self, &'static str> {
        let certificate =
            reqwest::Certificate::from_pem(pem).map_err(|_| "invalid callback CA certificate")?;
        self.client = client_builder()?
            .tls_certs_merge([certificate])
            .build()
            .map_err(|_| "invalid callback CA certificate")?;
        Ok(self)
    }

    /// Register this callback under the declared BSL name; no request data selects its destination.
    pub fn register(self, guards: Guards, name: String) -> Guards {
        guards.register(name.clone(), move |request| {
            let guard = self.clone();
            let name = name.clone();
            async move { guard.check(&name, request).await }
        })
    }

    pub async fn check(&self, name: &str, request: GuardRequest) -> GuardVerdict {
        let result = match tokio::time::timeout(self.deadline, self.invoke(name, request)).await {
            Ok(result) => result,
            Err(_) => Err("timeout"),
        };
        result.unwrap_or_else(|class| {
            eprintln!("external guard {name}: {class}");
            GuardVerdict::deny("Guard check unavailable")
        })
    }

    async fn invoke(
        &self,
        name: &str,
        request: GuardRequest,
    ) -> Result<GuardVerdict, &'static str> {
        let body = serde_json::to_vec(&serde_json::json!({
            "version": 1, "guard": name, "callable": request.callable, "args": request.args, "ctx": request.ctx
        })).map_err(|_| "request encoding")?;
        if body.len() > MAX_REQUEST {
            return Err("request too large");
        }
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, self.authorization.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| "transport failure")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("callback status");
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/json")
        {
            return Err("content type");
        }
        if response
            .headers()
            .get_all(CONTENT_ENCODING)
            .iter()
            .any(|v| v != "identity")
        {
            return Err("content encoding");
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
        {
            return Err("response too large");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "response transport failure")?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err("response too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        protocol::decode(&bytes)
    }
}

fn client_builder() -> Result<reqwest::ClientBuilder, &'static str> {
    // Verify against in-memory trust anchors; platform verification may block
    // on network/keychain services inside the callback deadline.
    let certificates = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|der| reqwest::Certificate::from_der(der.as_ref()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid bundled callback trust roots")?;
    Ok(reqwest::Client::builder()
        .tls_certs_only(certificates)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tls_tests;
