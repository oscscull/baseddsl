//! Aggregated SQLite-backed integration suite: one test binary instead of a dozen, so the
//! full runtime + sqlx(sqlite) + tokio stack is linked once rather than per file. Each
//! former `tests/<name>.rs` is now a module under `tests/sqlite/`; test names keep working
//! (filter by substring, or `--test sqlite_all`). Gated once here — the whole binary is
//! empty without the `sqlite` feature.
#![cfg(feature = "sqlite")]

#[path = "sqlite/bulk_integration.rs"]
mod bulk_integration;
#[path = "sqlite/cancel_safety.rs"]
mod cancel_safety;
#[path = "sqlite/computed_integration.rs"]
mod computed_integration;
#[path = "sqlite/distinct_integration.rs"]
mod distinct_integration;
#[path = "sqlite/idempotency_db.rs"]
mod idempotency_db;
#[path = "sqlite/key_composite_integration.rs"]
mod key_composite_integration;
#[path = "sqlite/key_integration.rs"]
mod key_integration;
#[path = "sqlite/migrate_apply.rs"]
mod migrate_apply;
#[path = "sqlite/raw_integration.rs"]
mod raw_integration;
#[path = "sqlite/serial_integration.rs"]
mod serial_integration;
#[path = "sqlite/sqlite_integration.rs"]
mod sqlite_integration;
#[path = "sqlite/streaming.rs"]
mod streaming;
