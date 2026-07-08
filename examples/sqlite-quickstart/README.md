# sqlite-quickstart

The thing to copy to start: a `.bsl` schema consumed through the **generated typed
client** running over the in-process **`Engine`** against a live **SQLite** database — no
socket, no server, no infra. One command builds and runs an end-to-end scenario.

```
cargo run
```

Expected output:

```
created order id-0 for Ada
paged 3 orders across 2 pages
soft-deleted then restored order id-0

end-to-end scenario passed
```

The process exits `0` only if every step passed, so it doubles as an end-to-end smoke
test (CI can just `cargo run`).

## What it exercises

The scenario in `src/main.rs` runs the full ordinary GET/PUT surface end-to-end:

1. **create → read-your-writes** — `place_order` inserts an `Order` and reads it back in
   its declared `OrderCard` shape (including the nested `placed_by { name, email }`
   sub-object) inside the same transaction.
2. **get** — `order_by_id` reads one order back by id.
3. **list + scope** — `my_orders` is a plain `list`, but the `Tenant` scope filters it to
   the caller's org; a different org sees none of the rows.
4. **keyset pagination** — `recent_orders` walks the whole set two rows at a time via the
   opaque cursor.
5. **soft-delete + restore** — `cancel_order` (a soft `delete`) tombstones a row and reads
   it back in its shape; `restore_order` lifts the tombstone.

## How it's wired

```
schema/*.bsl ──build.rs──▶ OUT_DIR/client.rs   (based gen client — typed Rust client)
             └─────────────▶ OUT_DIR/schema.sql  (based gen sql   — SQLite DDL)

src/main.rs: Compiled::load(schema) ─▶ Engine over SqliteDb ─▶ Client<InProcess>
```

- **No checked-in generated code.** `build.rs` runs the compiler front end
  (`Compiled::load`, the same one `based check`/`based serve` use) and emits the typed
  client + DDL into `OUT_DIR` on every build, so they can never drift from the schema. A
  broken schema fails the build with the diagnostics.
- **The `InProcess` transport bridge** (~20 lines in `main.rs`) is the whole of what an
  embedding app writes: the generated client defines the `Transport` trait, so the orphan
  rule keeps the bridge in the consumer crate. It serializes the typed input + `$ctx` to
  JSON, runs it through `Engine::call`, and decodes the `200` body — the same typed call
  an HTTP client makes, minus the loopback socket.
- **`$ctx`** (the request context — org, user) is a typed method argument the *app*
  supplies from its auth layer, never the caller (auth.md/D7).

## This is the SQLite slice

SQLite needs no live server (bundled, in-memory), so this example builds and runs
anywhere. The MariaDB and Postgres slices — the same scenario against those servers via
Docker — are the remaining Track-B sub-items (see the repo `PLAN.md`).

> Standalone crate, **outside** the cargo workspace (the root `Cargo.toml` `exclude`s
> `examples/`). It depends on the in-repo engine crates by path, so it always tracks the
> current engine, but `cargo test --workspace` never builds it.
