# based

A DB-first DSL and engine. Describe your data model, relations, queries, and
mutations in one small language (`.bsl`); the compiler generates the SQL, a typed
access layer, and a runnable service. MySQL/MariaDB is the primary target; SQLite
and Postgres are also supported.

Early and actively built — see [`PLAN.md`](PLAN.md) for status.

## Try it

Runnable quickstarts (one per database) are in [`examples/`](examples/) —
[`sqlite-quickstart`](examples/sqlite-quickstart/) runs in-memory with no setup:

```sh
cargo run   # from inside an example project
```

## Layout

- [`spec/`](spec/) — language design docs; start with [`spec/principles.md`](spec/principles.md).
- [`crates/`](crates/) — the Rust compiler + runtime workspace.
- [`examples/`](examples/) — runnable quickstart projects.

## License

[AGPL-3.0](LICENSE).
