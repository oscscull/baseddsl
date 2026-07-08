# mariadb-quickstart

The thing to copy to start on **MariaDB**: the *same* `.bsl` schema + end-to-end scenario as
[`sqlite-quickstart`](../sqlite-quickstart), consumed through the **generated typed client**
running over the in-process **`Engine`** — but pointed at a real MariaDB server instead of
bundled SQLite. One command builds and runs the scenario against your server.

```
# Point it at any MariaDB (this default matches the throwaway server below):
DATABASE_URL="mysql://root:based_test_pw@127.0.0.1:3307/based_test" cargo run
```

`DATABASE_URL` defaults to `mysql://root:based_test_pw@127.0.0.1:3307/based_test` if unset.
Bring up a throwaway server to run against:

```
docker run --rm -d --name based-mariadb -p 3307:3306 \
  -e MARIADB_ROOT_PASSWORD=based_test_pw -e MARIADB_DATABASE=based_test mariadb:11.4
```

Expected output (the engine mints real v4 UUIDs, so the id varies):

```
created order 3f2b… for Ada
paged 3 orders across 2 pages
soft-deleted then restored order 3f2b…

end-to-end scenario passed
```

The process exits `0` only if every step passed, so it doubles as an end-to-end smoke test.
It **resets its three tables on startup** (`DROP TABLE IF EXISTS` → recreate → seed), so it is
safe to re-run against the same server.

## What it exercises

The scenario in `src/main.rs` runs the full ordinary GET/PUT surface end-to-end — identical to
the SQLite slice: **create → read-your-writes** (in the declared `OrderCard` shape, including
the nested `placed_by { name, email }` sub-object) → **get** → **list + `Tenant` scope**
(a different org sees no rows) → **keyset pagination** → **soft-delete + restore**.

## How it's wired

```
schema/*.bsl ──build.rs──▶ OUT_DIR/client.rs   (based gen client — typed Rust client)
             └─────────────▶ OUT_DIR/schema.sql  (based gen sql   — MariaDB DDL)

src/main.rs: Compiled::load(schema) ─▶ Engine over ShardRouter/MariaDb ─▶ Client<InProcess>
```

The only differences from the SQLite slice are the **driver** (a pooled `ShardRouter`/`MariaDb`
over your live `DATABASE_URL`, not an in-memory `SqliteDb`), the **id generator** (`UuidGen` —
MariaDB's native `UUID` id columns reject non-uuid ids), and the **fixture ids** (real UUIDs,
for the same reason). The schema, the generated client, the `InProcess` transport bridge, and
every scenario assertion are byte-for-byte the SQLite example's — that is the point of the
reference: the engine is driver-agnostic, so an app moves between dialects by swapping the
driver + manifest `dialect`, nothing else.

- **No checked-in generated code.** `build.rs` runs the compiler front end (`Compiled::load`)
  and emits the typed client + MariaDB DDL into `OUT_DIR` on every build, so they can never
  drift from the schema. A broken schema fails the build with the diagnostics.
- **The `InProcess` transport bridge** (~20 lines in `main.rs`) is the whole of what an
  embedding app writes: the generated client defines the `Transport` trait, so the orphan rule
  keeps the bridge in the consumer crate.
- **`$ctx`** (org, user) is a typed method argument the *app* supplies from its auth layer,
  never the caller (auth.md/D7).

> Standalone crate, **outside** the cargo workspace (the root `Cargo.toml` `exclude`s
> `examples/`). It depends on the in-repo engine crates by path, so it always tracks the
> current engine, but `cargo test --workspace` never builds it.
