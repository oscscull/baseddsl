# syntax/transactions.md

Principles: 5 (nothing Turing-complete in the DSL — app logic lives in host Rust), 7 (engine
owns the tx boundary; caller supplies intent), 1 (dangerous is explicit + visible).

## Read-decide-write is not in the DSL

The DSL is a closed set of queries and mutations; a mutation's `tx { … }` (mutations.md) runs a
**static** set of writes atomically, but it can't branch on a value it read. Real
read-decide-write — read a row, decide in code, then write — is **host-language logic**, so it
lives in host Rust against an engine-owned transaction, never in the DSL (principle 5).

The seam is **embedded-only** (in-process): it runs a transaction over the same `Engine` an
embedding app already holds. It is not a wire feature — a transaction cannot span HTTP requests.

## The client is already transaction-ready

The generated client is `Client<T: Transport>` (calling.md). A transaction is that **same client
over a transaction-bound transport** — every generated method works inside a transaction with
**no per-callable codegen**. Inside a transaction, a query reads on the open transaction and a
mutation runs its writes on it (committing nothing); the engine commits or rolls back at the
boundary it owns.

## Isolation (shared by every rung)

Every entry point takes a `TxOptions` — the isolation level and access mode:

```rust
pub struct TxOptions { isolation: Isolation, access: AccessMode }   // Default: ReadCommitted + ReadWrite
pub enum Isolation  { ReadCommitted, RepeatableRead, Serializable }
pub enum AccessMode { ReadWrite, ReadOnly }
```

The level is applied per dialect through the `Dialect` seam (`based_codegen::Dialect::begin_transaction_sql`),
so the spelling can never drift from the compile target:

| dialect         | how the transaction opens |
|-----------------|---------------------------|
| Postgres        | `BEGIN ISOLATION LEVEL <level> READ WRITE\|READ ONLY` |
| MySQL / MariaDB | `SET TRANSACTION ISOLATION LEVEL <level>, READ WRITE\|READ ONLY` (applies to the next transaction), then the default `BEGIN` |
| SQLite          | no SQL-standard levels — the intent maps to the `BEGIN` locking mode: `Serializable` → `BEGIN EXCLUSIVE`, else read-only → `BEGIN DEFERRED`, else read-write → `BEGIN IMMEDIATE` |

`Serializable` is the optimistic-concurrency lever: the engine may abort a transaction with a
serialization failure, which the retrying rung re-runs (below).

## The three rungs

### Rung 1 — managed closure (the safe default)

The engine owns the boundary: **commit on `Ok`, roll back on `Err` or panic, always release.**

```rust
let out = client::transaction(&engine, TxOptions::default(), |tx| async move {
    let order = tx.order(OrderInput { id }, ctx).await?          // read
        .ok_or_else(|| TxError::app("no such order"))?;
    if order.total > budget {                                    // decide (host Rust)
        return Err(TxError::app("over budget"));                 // → rollback
    }
    tx.place_shipment(ShipmentInput { order: order.id }, ctx).await?;   // write
    Ok(order.total)
}).await?;   // committed
```

The closure receives a `Client<TxTransport>` and returns `Result<R, TxError>`. `TxError` converts
from a failed client call (`?`) and from `Engine::begin`; `TxError::app(e)` wraps an application
error the closure aborts (and rolls back) with.

**Retry variant** — `client::transaction_retrying(&engine, opts, Retry::on_serialization(max), |tx| …)`
re-runs the **whole closure** when the driver classifies the failure as a serialization/deadlock
abort (`DbErrorKind::Deadlock`, D65). Only the engine-owned form can auto-retry, because only it
owns the boundary — this is the `Serializable` payoff.

### Rung 2 — explicit handle (caller owns the lifetime)

Open-ended, for logic that doesn't fit one closure:

```rust
let txn = engine.begin(TxOptions::default()).await?;   // -> Transaction
let tx = txn.client();                                  // Client<TxTransport> bound to it
tx.place_order(input, ctx).await?;
txn.commit().await?;                                    // or txn.rollback().await?
```

**Dropping the handle without committing rolls back** — the `Tx` typestate guarantees it (a lost
handle, or a panic, never leaks a half-written transaction). `commit`/`rollback` are the explicit,
awaited forms.

### Rung 3 — bring-your-own transaction (`adopt`) — NOT YET BUILT (slice 3)

For interop with app code that already opened a transaction on the same driver:
`client::adopt(&mut caller_sqlx_tx)` will bind the generated calls to a transaction the caller
owns (the caller commits). This needs per-driver adapters (one per sqlx driver), so it is a
documented follow-on, **not yet implemented**.

## `for update` row locking — BUILT (slice 2, D119)

A `.bsl` `get`/`list` query requests pessimistic row locks with a **trailing `for update`
modifier**, after the clause list in a **block** query body:

```
query order_for_update(id) -> OrderRow { get Order where (id = $id) for update; }
```

`for update` is one compound keyword (like `group by` / `on conflict` / `hard delete`), so there is
no bare-token adjacency (principle 3). It sits after any `where`/`order`/`page`, before the `;` — the
block form is where the verb (`get`/`list`) and `where` are explicit, so a dangerous locking read is
written visibly (principle 1). Inline/bare query bodies do not carry it.

**Per-dialect lowering** (over the `Dialect::for_update_clause` seam, so the spelling can't drift from
the compile target): `FOR UPDATE`, appended last (after `ORDER BY`/`LIMIT`), on Postgres and the
MySQL/MariaDB family; **a no-op on SQLite** — SQLite has no row-level lock, but its transaction locks
the whole database (`BEGIN IMMEDIATE`/`EXCLUSIVE`, slice 1) and already serializes writers, so the lock
intent is honored at the transaction boundary rather than per row. The no-op is documented, never
silently misleading (principle 9).

**SQL-legal boundaries, enforced uniformly at compile time** (never a Postgres-only runtime failure —
the same discipline `distinct`'s E0312 uses): `FOR UPDATE` is rejected with `distinct` (**E0315** — a
deduped projection has no single base row to lock), on an aggregate/`group by` query (**E0316** — a
group locks no base row), on a query projecting a to-many nest (**E0317** — the json-agg subquery has
no lockable row), and on a `-> stream` query (**E0318** — a lock held across a long-lived stream is a
footgun).

**Compile-time confinement to transaction clients.** `SELECT … FOR UPDATE` outside a transaction locks
nothing useful (it releases immediately), so a `for update` query's generated method is **callable only
on a transaction-bound client**. A `TxBound` marker trait carries this: the locking methods are emitted
in `impl<T: Transport + TxBound> Client<T>`, and **only `TxTransport` implements `TxBound`** — the
auto-commit `Client<Embedded>` (and any wire client) does not, so calling `order_for_update` on it is a
**compile error**, not a silent no-op. Ordinary non-locking methods stay on `impl<T: Transport>` and
work in and out of a transaction. In a pure-wire build (no `based_runtime`, no `TxTransport`) the trait
is still declared but has no implementor, so the locking methods are simply uncallable — the module
compiles unchanged.

Deferred as clean micro-follow-ons (transactions.md): `for update nowait` / `for update skip locked`
(Postgres both; MariaDB 10.6+ both; SQLite none) — the MariaDB-version gating makes honest emission a
follow-up, not part of this slice.

## What shipped (slice 1)

`TxOptions`/`Isolation`/`AccessMode` (applied per dialect via the `Dialect` seam), `Engine::begin`
→ `Transaction` (rung 2), the managed `client::transaction` / `transaction_retrying` (rung 1), the
`Transaction::client()` accessor, and the `TxTransport` transaction-bound transport the generated
client runs on. The central runtime refactor factors dispatch so a request runs against a
**provided open transaction** (`dispatch_on` / `run_mutation_on`) instead of a fresh
auto-committing checkout; the auto-commit path is unchanged. Implementation: D118.

## What shipped (slice 2)

The `for update` locking-read modifier + its `TxBound` compile-time confinement (the built section
above): parser (trailing `for update` in the block query body) → AST (`Statement.for_update`) → sema
(the four E-codes E0315–E0318) → codegen (`Dialect::for_update_clause` seam + `SELECT … FOR UPDATE`
emission; the `TxBound` marker trait + confined `impl<T: Transport + TxBound> Client<T>` block, with
`impl TxBound for TxTransport` under the embedded bridge) → fmt round-trip → LSP keyword completion.
Proven live on Postgres (two concurrent transactions: B's `for update` read blocks until A commits, then
observes A's committed write). Implementation: D119.

Slice 3 (BYO `adopt` adapters, plus the flagship axum-helpdesk read-decide-write use case) remains the
documented follow-on above.
