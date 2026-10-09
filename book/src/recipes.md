# Working with scopes and transactions

## Select one ownership alternative or require both axes

```bsl
scope Tenant (org: uuid = $ctx.org)
scope Author (owner: uuid = $ctx.user)

@scope Tenant
@scope Author
Post {
  id: Id
  org: uuid
  owner: uuid
  body: text
  @index org
  @index owner
}
query org_posts() -> Post[] scoped Tenant { list Post order (id); }
query my_posts() -> Post[] scoped Author { list Post order (id); }

@scope Tenant, Author
Draft {
  id: Id
  org: uuid
  owner: uuid
  body: text
  @index(org, owner)
}
query my_org_drafts() -> Draft[] scoped Tenant, Author { list Draft order (id); }
```

Two decorators are **OR alternatives at declaration time**: `org_posts` chooses
the org axis; `my_posts` chooses the owner axis. Neither query dynamically returns
the union of both sets. One decorator with comma-separated names is **AND**:
drafts require both predicates, and a callable naming only Tenant is rejected.
Naming both axes on the OR model also intersects their row sets.

Required context must be present; otherwise execution fails with `missing_ctx`.
Creates auto-set scope columns from context and cannot explicitly assign them.
A create must satisfy a whole declared alternative and supply required non-null
ownership fields. `unscoped("reason")` explicitly forfeits scope injection; host
authentication must authorize that crossing. Incorrect host context can select
another owner's rows. Raw subqueries and whole raw bodies own their internal
filters; see [raw boundaries](reference.md#raw).

## Keep a write set atomic and enforce a state transition

```bsl
mutation create_pair(parent_name: text, child_name: text) -> ItemView scoped Author {
  tx {
    create Item { name = $parent_name } as parent;
    create Item { name = $child_name, parent = $parent.id };
  }
}
```

The static write set commits together or rolls back together. `$parent.id` reads
the created row within that transaction. The DSL does not branch on a read value;
read-decide-write logic belongs in a host transaction.

For a guarded transition, use a conditional write:

```bsl
mutation rename_item(id: Id, name: text, expected_name: text) -> ItemView guard caller_can_rename scoped Author {
  update Item where (id = $id and name = $expected_name) { name = $name };
}
```

The named guard decides permission **before** the write transaction. Its state
reads use another connection, including for adopted transactions; they cannot see
uncommitted caller state or hold its row locks. The conditional predicate makes
the expected state part of the write. A stale/absent/out-of-scope row returns
`404 not_found` with no transition. A preflight denial returns `403 guard_denied`.
Guard registration is required before an engine/service can start.

Use [host transactions](reference.md) for locking
read-decide-write. `for update` requires a transaction-bound client and gives row
locks on supported server databases; it is a documented no-op on SQLite, whose
transaction locking differs. An HTTP transaction cannot span requests.

## Add SQL while naming the boundary you own

```bsl
shape ItemLength from Item {
  id
  length = raw`length(name)`
}
query item_lengths() -> ItemLength[] scoped Author { list Item order (id); }
```

Root and modeled joins keep injected scope/soft-delete predicates. The raw
expression is opaque: any subquery you place inside it owns its own table filters.
For example, `(SELECT COUNT(*) FROM item)` can count all owners even if outer
rows are confined. Prefer modeled relations, or explicitly filter internal tables.

A whole raw body must use a flat shape and opt out for a scoped model:

```bsl
shape ItemFlat from Item { id, name }
query named_items(owner: uuid, name: text) -> ItemFlat[] unscoped("host-authorized raw lookup") {
  raw`SELECT id, name FROM {table} WHERE owner = ${owner} AND name = ${name}`;
}
```

`${owner}` and `${name}` bind parameters; they are not SQL string concatenation.
The host must derive and authorize `owner`. You own all scope and soft-delete
filters, ordering, and dialect portability in this query. Declared result types
do not verify the SQL's authorization or its returned columns. A raw body cannot
promise `scoped`, infer untyped parameters, return nested shapes, or stream.

## Integrate an existing pool and caller-owned transaction

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
[database TLS](tls.md) and [deployment](deployment.md).

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
feature. See [adoption semantics](reference.md)
for SQLite/MariaDB equivalents and engine-owned alternatives. Caller-owned
transactions have no automatic whole-transaction retry.

## Retry without confusing replay with transaction retries

For HTTP retries, see [idempotency](idempotency.md).

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
the caller owns their boundary.
