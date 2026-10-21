# Project and connection configuration

Without a root argument, CLI commands find the nearest ancestor `based.toml`.
An explicit root uses only that directory's manifest. Schema, migration, output,
and relative SQLite paths resolve from the selected manifest directory.

```toml
dialect = "sqlite"
root = "schema"
[ schema ]
id = "uuid"
foreign_keys = "none"
```

Dialects are `sqlite`, `mariadb`/`mysql`, and
`postgres`/`postgresql`. ID strategies are `uuid`, `ulid`,
and `serial`; FK defaults are `all` or `none`. Unknown options
fail so misspellings cannot silently select a default.

Live commands select connections in this order:

1. Explicit `--database-url` flags.
2. Process `BASED_DATABASE_URL`, then `DATABASE_URL`.
3. The selected project's `.env`, in the same variable order.

Empty values fail. A URL variable can contain comma-separated shard URLs; SQLite
serve accepts one file.

A SQLite connection is a file path (`app.db`, `/srv/app.db`), `:memory:`, or a
`sqlite:` URL: `sqlite:app.db`, `sqlite://app.db`, `sqlite:///srv/app.db`,
`sqlite::memory:`. The file is created if missing. Any other scheme, and URL
query parameters, are rejected. Offline checking, formatting, generation, and migration
planning do not require a connection or read `.env`.

Keep credentials, database files, and `.env` out of source control.
Application-owned pools use their own connection configuration.
