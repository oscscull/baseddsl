//! External callback contracts against controlled sockets and persisted SQLite effects.
#![cfg(all(feature = "external-guards", feature = "sqlite", feature = "serve"))]
#[path = "external_guards/failures.rs"]
mod failures;
#[path = "external_guards/harness.rs"]
mod harness;
#[path = "external_guards/success.rs"]
mod success;

#[path = "external_guards/fixture.rs"]
mod fixture;
