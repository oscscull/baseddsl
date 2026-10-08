# Retry without confusing replay with transaction retries

Task: resolve an uncertain committed request or retry a database-aborted
transaction. These are different boundaries.

For a standalone logical mutation, retain the same payload, trusted context,
and globally unique key when retrying:

```sh
python3 tutorial/client.py editor /m/create_item '{"name":"Retry example","parent":null}' --key UNIQUE_OPERATION_KEY
```

The [standalone tutorial](../standalone-tutorial.md) verifies database replay
across restart. The authoritative [idempotency contract](../standalone-idempotency.md)
defines store boundaries: database key/effect/response commit atomically; memory
or out-of-band response recording has a post-commit failure gap. Keys are scoped
by callable within the store/database, not tenant. Changed args/context conflict;
guards check current permission before replay. Expiry/deletion permits execution
again. No store deduplicates arbitrary external effects or guarantees success.

For an embedded transaction, the generated client retries a **whole closure** on
classified serialization/deadlock aborts. Using the tutorial's owner context:

```rust
let rows = client::transaction_retrying(
    &engine,
    based_runtime::TxOptions::default().serializable(),
    client::Retry::on_serialization(3),
    |tx| {
        let owner = owner.clone();
        async move {
            Ok(tx.items(client::ItemsInput {}, client::ItemsCtx { owner }).await?)
        }
    },
).await?;
```

`owner` is the host's authenticated `client::Uuid`; this snippet assumes the
tutorial's generated client and engine. The closure may run more than once, so
keep nontransactional external effects out of it. Application errors are not
automatic retry signals. Adopted caller-owned transactions cannot auto-retry;
the caller owns their boundary. See
[transaction retry semantics](../../spec/syntax/transactions.md#rung-1--managed-closure-the-safe-default)
and [the classified-abort test](../../crates/based-runtime/tests/core/embed.rs).
