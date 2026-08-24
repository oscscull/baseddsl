//! Aggregated HTTP-serve suite: the `serve`-gated tests in one binary. Empty without the
//! `serve` feature.
#![cfg(feature = "serve")]

#[path = "serve/http.rs"]
mod http;
#[path = "serve/streaming_client.rs"]
mod streaming_client;
