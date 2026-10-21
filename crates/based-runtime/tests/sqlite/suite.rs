// The SQLite suite's modules, shared by `sqlite_all` and `rusqlite_all`.

#[path = "bulk_integration.rs"]
mod bulk_integration;
#[path = "cancel_safety.rs"]
mod cancel_safety;
#[path = "computed_integration.rs"]
mod computed_integration;
#[path = "conditional_transition.rs"]
mod conditional_transition;
#[path = "distinct_integration.rs"]
mod distinct_integration;
#[path = "filter_projection_integration.rs"]
mod filter_projection_integration;
#[path = "fixtures.rs"]
mod fixtures;
#[path = "idempotency_db.rs"]
mod idempotency_db;
#[path = "in_list_integration.rs"]
mod in_list_integration;
#[path = "json_roundtrip.rs"]
mod json_roundtrip;
#[path = "key_composite_integration.rs"]
mod key_composite_integration;
#[path = "key_integration.rs"]
mod key_integration;
#[path = "migrate_apply.rs"]
mod migrate_apply;
#[path = "migrate_data.rs"]
mod migrate_data;
#[path = "optional_ctx_integration.rs"]
mod optional_ctx_integration;
#[path = "optional_filter_integration.rs"]
mod optional_filter_integration;
#[path = "raw_integration.rs"]
mod raw_integration;
#[path = "serial_integration.rs"]
mod serial_integration;
#[path = "sqlite_integration.rs"]
mod sqlite_integration;
#[path = "streaming.rs"]
mod streaming;
