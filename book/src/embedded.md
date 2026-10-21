# A small Rust application

After [installing Based](installation.md), run these commands in an empty directory:

```sh
based init --mode embedded
based migrate apply --database-url local.db
cargo run
```

The application creates a parent and child, then prints both through the generated
typed client. SQLite uses a local file; no database server is required.
`build.rs` generates the client from the schema on every `cargo build`; see
[generated clients](generation.md).

Read `schema/item.bsl` first. `Item` has an optional parent, an owner scope,
and an `ItemView` projection containing its parent's ID and name. The generated
call accepts separate input and context:

```rust
let rows = api
    .items(client::ItemsInput {}, client::ItemsCtx { owner: session.owner() })
    .await?;
```

The host derives the owner from its authenticated session. The engine filters
modeled reads and writes by that scope. The sample uses a fixed local identity;
a production application must authenticate it. A missing lookup returns `None`.

The Rust entry point loads the schema and opens the database. The demo owns the
calls; session and lookup code handle context and optional results. Read those
small files before replacing the sample with your application.

## Ship one binary

The schema is compiled into the executable, so a bundled app needs no
`schema/` folder at runtime. `build.rs` checks it (a broken schema fails
`cargo build`) and embeds it:

```rust
mod schema {
    include!(concat!(env!("OUT_DIR"), "/schema.rs"));
}

let compiled = based_runtime::Compiled::from_sources(schema::MANIFEST, schema::SOURCES)?;
```

Without `based-build`, pass the manifest text and `(name, text)` pairs
yourself:

```rust
const SCHEMA: &[(&str, &str)] = &[("task.bsl", include_str!("../schema/task.bsl"))];
let compiled = Compiled::from_sources(include_str!("../based.toml"), SCHEMA)?;
```

`from_sources` runs the same check as `Compiled::load`: errors fail and cite
the embedded name, and warnings do not.

## Rename a field

Change `name: text` to `title: text @was("name")`. In projections use
`name = title`, including the parent projection, to preserve the public field.
Change assignments from `name = $name` to `title = $name`.

```sh
based check
based migrate gen . rename_item_title
based migrate verify
based migrate render . --number 2
based migrate apply --database-url local.db
cargo run
```

Review the rendered migration before applying it; `cargo run` picks up the
schema change without a separate generation step. `@was` produces a rename
rather than a drop/add, preserving existing values and relation IDs. Commit the
migration files and Cargo.lock.

Use a new database file for a fresh run. For MariaDB or PostgreSQL, select
`--dialect mariadb` or `--dialect postgres` at initialization and export
`DATABASE_URL` for a database you provide.

## Choose the SQLite driver

`based-runtime` has two SQLite drivers with the same behaviour: pools,
transactions, binds, migrations, idempotency and errors.

| Feature | Backend | Driver | `libsqlite3-sys` |
| --- | --- | --- | --- |
| `sqlite` | `SqliteBackend` | sqlx 0.9 | 0.37.x |
| `sqlite-rusqlite` | `RusqliteBackend` | rusqlite 0.39–0.40 | 0.37.x or 0.38.x |

Cargo links one `libsqlite3-sys` per build, so pick the driver whose range
contains the one your application already uses. An application on rusqlite
0.40 uses `sqlite-rusqlite`:

```toml
based-runtime = { version = "0.1", default-features = false, features = ["sqlite-rusqlite"] }
```

`RusqliteBackend` has the same constructors as `SqliteBackend` (`in_memory`,
`open`, `open_with_pool`). `from_connection` and `from_connector` take
connections you configured, and `with_connection` runs your own rusqlite code
on the engine's connection. To include engine calls in your transaction, run
`BEGIN` on your connection and pass it to the generated `adopt_rusqlite`
(your crate's `sqlite-rusqlite` feature forwards to
`based-runtime/sqlite-rusqlite`); the calls fail if no transaction is open. Both features bundle SQLite, version
3.51.3 or later.
