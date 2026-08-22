# syntax/transactions.md

Principles: 5 (nothing Turing-complete in the DSL — app logic lives in host Rust), 7 (engine
owns the tx boundary; caller supplies intent), 1 (dangerous is explicit + visible).

## Read-decide-write is not in the DSL

The DSL is a closed set of queries and mutations; a mutation's `tx { … }` (mutations.md) runs a
**static** set of writes atomically, but it can't branch on a value it read. Real
read-decide-write — read a row, decide in code, then write — is **host-language logic**, so it
lives in host Rust against an engine-owned transaction, never in the DSL (principle 5).

Within a `tx`, a `create … as name` **re-selects its written row**, so a later step's
`$name.field` reads the row the database actually wrote — read-your-writes *inside the
transaction*, seeing DB defaults, engine timestamps, and DB-generated (`serial`) ids as
written (mutations.md, D124). This is threading a committed value between static steps, not
branching on it — the write set is still fixed at compile time.

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

### Rung 3 — bring-your-own transaction (`adopt`) — BUILT (slice 3, D120)

For interop with app code that already opened a transaction on the same driver: a per-driver
`client::adopt_<driver>(&engine, &mut caller_sqlx_tx)` binds the generated calls to a transaction
the **caller** owns, so baseddsl's writes commit atomically with the caller's own raw writes on
that transaction.

```rust
let mut tx = pool.begin().await?;                       // caller's own sqlx transaction
sqlx::query("INSERT INTO audit_log …").execute(&mut *tx).await?;   // a raw app write
{
    let api = client::adopt_postgres(&engine, &mut tx); // adopt it (borrowed)
    api.order_for_update(input, ctx).await?;            // a `for update` locking read works
    api.place_shipment(input, ctx).await?;              // a baseddsl write, on the same tx
}                                                       // the adopted client drops — tx untouched
tx.commit().await?;                                     // the caller commits BOTH writes atomically
```

**`adopt` never begins, commits, or rolls back — the caller owns the boundary.** Dropping the
adopted client leaves the caller's transaction untouched (unlike rung 2's `Transaction`, which
drop-rolls-back); the caller commits or rolls back it itself, and the raw + baseddsl writes land
(or vanish) together. There is no `transaction_retrying` here — auto-retry needs an engine-owned
boundary. The adopted client is `TxBound`, so `for update` reads work through it (the lock is held
to the caller's boundary).

**One constructor per driver, for the compile target.** `sqlx`'s `Transaction<'_, DB>` types don't
unify across drivers, so there is one constructor per driver — `adopt_postgres` / `adopt_sqlite` /
`adopt_mariadb` (the MySQL/MariaDB family shares one `sqlx::MySql` driver) — each `#[cfg]`-gated on
the consumer's matching driver feature (forward it to `based-runtime/<driver>`). The generated
client emits exactly the one for its compile-target dialect. Under the hood each wraps the caller's
*borrowed* `sqlx::Transaction` in a per-driver adapter implementing baseddsl's `DbRead` read+execute
seam (never the owning `Tx`), run through the same dispatch core as the engine-owned rungs.

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

**Wait modes — `for update nowait` / `for update skip locked`.** A `for update` read may carry an
optional wait mode saying what to do when a target row is already locked by another transaction:

```
query claim_next(id)  -> JobRow { get  Job where (id = $id)  for update nowait; }
query take_batch(max) -> JobRow[] { list Job where (id > $max) for update skip locked; }
```

- **`for update nowait`** — fail immediately (a lock-not-available error) instead of blocking.
- **`for update skip locked`** — omit already-locked rows instead of blocking (the classic
  work-queue claim: each worker grabs a different unlocked batch).
- Plain **`for update`** (no wait mode) is unchanged — it blocks until the lock is released.

`for update`, `for update nowait`, and `for update skip locked` are the three spellings; the wait
words follow the `for update` keyword, before the `;`. They ride the **same** compile-time boundaries
as plain `for update` (the E0315–E0318 set above) — a wait mode adds no new legal or illegal
combination.

**Per-dialect lowering** (the same `Dialect::for_update_clause` seam): Postgres and the MySQL/MariaDB
family spell `FOR UPDATE NOWAIT` / `FOR UPDATE SKIP LOCKED` (`NOWAIT` needs MySQL 8.0+ / MariaDB
10.3+, `SKIP LOCKED` MySQL 8.0+ / MariaDB 10.6+ — modern servers; our `mariadb:11.4` target has
both). On **SQLite** every wait mode is the **same documented no-op** as plain `for update`: SQLite
has no row-level lock, so there is no already-locked row to skip or fail fast on — its whole-database
transaction lock serializes writers at the boundary regardless. Consistent with the plain-`for update`
no-op, never silently misleading (principle 9).

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

## What shipped (slice 3) — the feature is COMPLETE

The BYO `adopt` interop rung (the built section above): per-driver borrowed `DbRead` adapters
(`AdoptedPg` / `AdoptedSqlite` / `AdoptedMaria`) over a caller-owned `sqlx::Transaction`, wrapped in the
`AdoptedTransport<D>` transport and run through the same dispatch core (`dispatch_on` generalized from a
`Tx` to any `DbRead`, so an engine-owned transaction and a borrowed adopted one take the identical path);
the generic adopted `Transport` + `TxBound` impls and the one per-dialect `#[cfg]`-gated `adopt_*`
constructor in codegen; `based_runtime` re-exports `sqlx` so a consumer names the same driver types.
Proven live (raw app write + baseddsl `for update` read + mutation on one caller-owned transaction, atomic
on commit, discarded together on rollback) on Postgres and SQLite, and demonstrated end-to-end in the
flagship axum-helpdesk (`resolve_with_audit` → `POST /tickets/{id}/resolve`, in the smoke). Implementation:
D120. With this the three-rung transaction seam (D118–D120) is complete.

The `for update nowait` / `for update skip locked` wait modes shipped as the documented micro-follow-on
(the wait-mode section under `for update` above): parser (the optional `nowait` / `skip locked` after
`for update`) → AST (`Statement.for_update: Option<LockWait>`) → codegen (`Dialect::for_update_clause`
spells each mode, no-op on SQLite for all) → fmt/LSP round-trip. Proven live on Postgres (`skip locked`
returns the unlocked rows, `nowait` errors fast on a locked row) and as a no-op on SQLite. The
three-rung transaction seam plus its locking-read modifiers are now complete.
