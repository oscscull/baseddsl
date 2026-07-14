//! Multi-tenant support desk on based + Postgres.
//!
//! The service embeds the engine: `app` wires it over the app's own sqlx pool and
//! registers the close policy; `auth` trades `Authorization: Bearer` for a typed
//! session through the client itself; `routes` is the HTTP surface, one typed
//! client call per handler.
//!
//! `client` is the verbatim output of `based gen client -o src/client.rs --embedded`,
//! checked in as a reviewable artifact: the typed API surface (inputs, per-request
//! `$ctx` structs, output shapes, enums, typed ids) plus the in-process `Embedded`
//! transport over `based_runtime::Engine`. Regenerate after a schema change; never
//! edit by hand.

pub mod app;
pub mod auth;
pub mod client;
pub mod routes;

pub use app::App;
