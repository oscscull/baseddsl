//! Authenticated external guard transport. Configuration is operator-owned and validated
//! before registration; request metadata cannot change endpoints, secrets or limits.
mod config;
mod response;
pub use config::ConfigError;

use crate::{GuardRequest, GuardVerdict, Guards};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;

const MAX_REQUEST: usize = 256 * 1024;
const MAX_RESPONSE: usize = 16 * 1024;

/// A validated callback. Debug deliberately excludes the endpoint and credential.
#[derive(Clone)]
pub struct HttpGuard {
    name: String,
    endpoint: reqwest::Url,
    authorization: reqwest::header::HeaderValue,
    deadline: Duration,
    client: reqwest::Client,
}

impl std::fmt::Debug for HttpGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpGuard")
            .field("name", &self.name)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl HttpGuard {
    /// Validate and create a callback. HTTPS always verifies certificates and hostnames.
    /// Plain HTTP requires an explicit development exception and a literal loopback host.
    pub fn new(
        name: String,
        endpoint: &str,
        secret: &str,
        deadline: Duration,
        allow_loopback_http: bool,
    ) -> Result<Self, ConfigError> {
        let (endpoint, authorization) =
            config::validate(&name, endpoint, secret, deadline, allow_loopback_http)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(deadline)
            .build()
            .map_err(|_| ConfigError("cannot initialize verified guard HTTP transport"))?;
        Ok(Self {
            name,
            endpoint,
            authorization,
            deadline,
            client,
        })
    }

    /// Add this callback to the ordinary pre-write guard registry.
    pub fn register(self, guards: Guards) -> Guards {
        let name = self.name.clone();
        let callback = Arc::new(self);
        guards.register(name, move |request| {
            let callback = Arc::clone(&callback);
            async move { callback.check(request).await }
        })
    }

    async fn check(&self, request: GuardRequest) -> GuardVerdict {
        match tokio::time::timeout(self.deadline, self.invoke(request)).await {
            Ok(Ok(verdict)) => verdict,
            failure => {
                let class = match failure {
                    Ok(Err(class)) => class,
                    _ => "timeout",
                };
                eprintln!("external guard `{}`: {class}", self.name);
                GuardVerdict::deny("Guard check unavailable")
            }
        }
    }

    async fn invoke(&self, request: GuardRequest) -> Result<GuardVerdict, &'static str> {
        let body = serde_json::to_vec(&serde_json::json!({
            "version": 1, "guard": self.name, "callable": request.callable,
            "args": request.args, "ctx": request.ctx,
        }))
        .map_err(|_| "request encoding")?;
        if body.len() > MAX_REQUEST {
            return Err("request size");
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
            .map_err(transport_error)?;
        response::validate_headers(&response)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if chunk.len() > MAX_RESPONSE - bytes.len() {
                return Err("response size");
            }
            bytes.extend_from_slice(&chunk);
        }
        response::decode(&bytes)
    }
}

fn transport_error(error: reqwest::Error) -> &'static str {
    match error.is_timeout() {
        true => "timeout",
        false => "transport",
    }
}

#[cfg(all(test, feature = "serve"))]
mod tls_tests;
