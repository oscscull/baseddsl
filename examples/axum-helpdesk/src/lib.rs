//! Multi-tenant support desk on based + Postgres.
//!
//! `client` is the verbatim output of `based gen client -o src/client.rs --embedded`,
//! checked in as a reviewable artifact: the typed API surface (inputs, per-request
//! `$ctx` structs, output shapes, enums, typed ids) plus the in-process `Embedded`
//! transport over `based_runtime::Engine`. Regenerate after a schema change; never
//! edit by hand.

pub mod client;
