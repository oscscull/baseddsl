# based

A DB-first DSL and engine. Describe models, relations, queries, and mutations in
short, readable `.bsl` files independent of your host language. Based generates
SQL, typed async Rust clients, and a standalone HTTP service. See the
[v1 support and compatibility policy](docs/support-policy.md) for the tested
databases, deployment paths, and current limits.

**[Language reference →](docs/reference.md)** — every feature and its syntax, on one page.

WIP — progress and open work are tracked in [GitHub issues](https://github.com/oscscull/baseddsl/issues).

## Try it

Runnable quickstarts (one per database) are in [`examples/`](examples/) —
[`sqlite-quickstart`](examples/sqlite-quickstart/) runs in-memory with no setup:

```sh
cargo run   # from inside an example project
```

## Layout

- [`docs/reference.md`](docs/reference.md) — the language reference (what to write).
- [`spec/`](spec/) — language design docs; start with [`spec/principles.md`](spec/principles.md).
- [`crates/`](crates/) — the Rust compiler + runtime workspace.
- [`examples/`](examples/) — runnable quickstart projects.

## License

Based source code, including the Rust crates and VS Code extension, is licensed
under [AGPL-3.0-only](LICENSE). See [generated-output terms](LICENSE-GENERATED.md)
for Rust clients, SQL, OpenAPI documents, and migration artifacts produced from
your schemas. Your schemas and data remain yours.
