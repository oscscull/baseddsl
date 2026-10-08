# based

A DB-first DSL and engine. Describe models, relations, queries, and mutations in
short, readable `.bsl` files independent of your host language. Based generates
SQL and typed async Rust clients, with embedded Rust and standalone HTTP execution.
See the [v1 support and compatibility policy](docs/support-policy.md) for tested
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
- [`docs/compile-time.md`](docs/compile-time.md) — build profiles, feature boundaries, and measured compilation costs.
- [`spec/`](spec/) — language design docs; start with [`spec/principles.md`](spec/principles.md).
- [`crates/`](crates/) — the Rust compiler + runtime workspace.
- [`examples/`](examples/) — runnable quickstart projects.

For certificate-verified Postgres and MariaDB connections, see
[database TLS configuration](docs/database-tls.md).

Standalone deployment requires a [trusted authentication edge](docs/standalone-deployment.md)
that strips caller-supplied context/shard headers and prevents direct listener access.

Standalone keyed mutation storage and explicit table setup are documented in
[standalone idempotency](docs/standalone-idempotency.md).
For custom standalone permission checks, run the
[TypeScript guard example](examples/standalone-typescript-guards/README.md);
[embedded Rust guards](examples/axum-helpdesk/README.md) remain equally supported.

## License

Based source code, including the Rust crates and VS Code extension, is licensed
under [AGPL-3.0-only](LICENSE). See [generated-output terms](LICENSE-GENERATED.md)
for Rust clients, SQL, OpenAPI documents, and migration artifacts produced from
your schemas. Your schemas and data remain yours.

For client-generator changes, run the separate [fresh external consumer gates](docs/generated-consumer-contracts.md)
in addition to the workspace and live checks.
