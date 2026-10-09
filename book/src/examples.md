# Larger applications

The initializer teaches a first run. These examples show larger workloads:

| Example | Use it to learn |
|---|---|
| [SQLite quickstart](../../examples/sqlite-quickstart/) | Typed IDs, pagination, scopes, soft deletion |
| [MariaDB quickstart](../../examples/mariadb-quickstart/) | The same client against a server database |
| [PostgreSQL quickstart](../../examples/postgres-quickstart/) | PostgreSQL connections and migrations |
| [Axum helpdesk](../../examples/axum-helpdesk/) | A multi-tenant host app with authentication, guards, streaming, and optional Redis |
| [TypeScript guards](../../examples/standalone-typescript-guards/) | An authenticated HTTP edge and external permission callbacks |
| [Commerce schema](../../spec/examples/commerce/) | A broader reference schema |

For a quickstart, change to its directory, provide `DATABASE_URL` for a disposable
server database (or a SQLite file), apply its migration, then run Cargo:

```sh
based migrate apply
cargo run
```

The helpdesk needs PostgreSQL and JWT configuration. Its reset/seed smoke is for
a disposable database only; read its manifest and entry point before running it.
Use `make ci-example-helpdesk` with the local test databases for the smoke.

Scopes protect modeled operations only when the host supplies trusted context.
Raw subqueries and host SQL own their predicates. An absent optional owner can
match unowned rows; it is not an automatic public-only filter.
