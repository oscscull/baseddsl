# A small Rust application

After [installing Based](installation.md), run these commands in an empty directory:

```sh
based init --mode embedded
based migrate apply --database-url local.db
cargo run
```

The application creates a parent and child, then prints both through the generated
typed client. SQLite uses a local file; no database server is required.

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

## Rename a field

Change `name: text` to `title: text @was("name")`. In projections use
`name = title`, including the parent projection, to preserve the public field.
Change assignments from `name = $name` to `title = $name`.

```sh
based check
based gen all
based migrate gen . rename_item_title
based migrate verify
based migrate render . --number 2
based migrate apply --database-url local.db
based gen all --check
cargo run
```

Review the rendered migration before applying it. Generation does not alter the
database. `@was` produces a rename rather than a drop/add, preserving existing
values and relation IDs. Commit the generated client, migration files, and Cargo.lock.

Use a new database file for a fresh run. For MariaDB or PostgreSQL, select
`--dialect mariadb` or `--dialect postgres` at initialization and export
`DATABASE_URL` for a database you provide.
