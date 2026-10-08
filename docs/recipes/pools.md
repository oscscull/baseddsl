# Integrate an existing pool and caller-owned transaction

Task: let the app and engine share the app's existing PostgreSQL pool. In the
advanced helpdesk wiring, with loaded `compiled` schema and registered `guards`:

```rust
let engine = based_runtime::Engine::with_guards(
    compiled,
    based_runtime::PgRouter::from_pool(pool.clone()),
    based_runtime::id::UuidGen,
    guards,
)?;
```

Pool settings and connection budget remain the app's responsibility. See the
[working wiring](../../examples/axum-helpdesk/src/app.rs), rather than opening
a second pool just for the engine. Server/TLS provisioning is in
[database TLS](../database-tls.md) and [deployment](../standalone-deployment.md).

To join a baseddsl call to an existing SQLx transaction, use its generated adapter:

```rust
let mut tx = pool.begin().await?;
{
    let api = client::adopt_postgres(&engine, &mut tx);
    api.set_status(input, ctx).await?;
}
tx.commit().await?;
```

This excerpt uses the helpdesk's typed `SetStatusInput` and `SetStatusCtx`.
The adopted client never begins, commits, or rolls back; dropping it leaves the
transaction to the caller. Raw host writes and modeled calls share the boundary,
but the host's SQL owns its own authorization/filters. Guards remain preflight
on separate connections. The [resolve-with-audit example](../../examples/axum-helpdesk/src/app.rs)
shows explicit rollback and a locking read in the same caller transaction.

The generated dialect-specific adapter requires the consumer's matching driver
feature. See [adoption semantics](../../spec/syntax/transactions.md#rung-3--bring-your-own-transaction-adopt)
for SQLite/MariaDB equivalents and engine-owned alternatives. Caller-owned
transactions have no automatic whole-transaction retry.
