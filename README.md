# Based

Based is a database definition language and execution engine. Keep models,
relations, queries, and mutations in readable `.bsl` files, then call the checked
access layer through a typed async Rust client or a standalone HTTP service.

It is for backend teams that want a reviewable database access definition alongside
their application. The engine runs the queries and owns their database lifecycle;
your application supplies connections, authenticated context, and host guard code.
See the [support boundary](docs/support-policy.md) before choosing it for a project.

```bsl
Item {
  id: Id
  name: text
}

query items() -> Item[] { list Item order (id); }
```

With the generated client and an embedded engine wired into your Rust application:

```rust
let api = client::embedded(&engine);
let rows: Vec<client::Item> = api.items(client::ItemsInput {}, ()).await?;
```

The compiler checks the schema/access definitions and generates the client, SQL,
and optional OpenAPI description. Embedded calls execute through the engine in
process. Standalone calls reach the same engine through HTTP; they do not execute
an emitted SQL file as a separate application. Business logic stays in host code.

## Start here

**Current status:** source version `0.1.12`, an evaluation candidate on the path to
v1. The [release tracker](https://github.com/oscscull/baseddsl/issues/43) contains
remaining gates. A successful dry run does not imply an owner-approved public
release. Use [versioned installation](docs/installation.md) for native CLI/LSP
artifacts, a matching VS Code extension, or explicitly pinned Git source/library
installation. Available assets and exact host prerequisites are described there.

Choose one starting path. SQLite uses a **local file** with an explicit migration
step and needs no database server. The embedded path also needs Rust/Cargo/Git and
platform C build tools; the standalone demo needs Python's standard library.

| Embedded Rust | Standalone HTTP |
|---|---|
| [Embedded tutorial](docs/embedded-tutorial.md) | [Standalone walkthrough](docs/initializing.md#standalone-walkthrough) |
| `based init --mode embedded` | `based init --mode standalone` |
| Apply the initial migration, then `cargo run`. | Apply the initial migration, then run the Python HTTP demo. |
| Your app calls the generated typed client in process. | Other applications call the service's query/mutation routes. |

Run init **inside an empty directory** and follow its two printed commands.
Both paths demonstrate create/read without editing code. Generation and migration
planning are offline; database migration application is always a separate step.
The [initializer guide](docs/initializing.md) also covers schema updates and safe
MariaDB/Postgres configuration.

## Documentation map

Follow these by purpose rather than treating the design specification as a setup
tutorial:

| Purpose | Read next |
|---|---|
| Install and evaluate | [Versioned installation](docs/installation.md), then either starting path above |
| Edit a schema and regenerate | [Initializer update cycle](docs/initializing.md#change-the-schema-explicitly), [generated artifact lifecycle](docs/generated-artifacts.md), [project configuration](docs/project-configuration.md) |
| Look up usable syntax | [Language reference](docs/reference.md) |
| Understand detailed semantics and design priorities | [Specification index](spec/README.md), [principles](spec/principles.md), and its syntax chapters |
| Configure a real standalone deployment | [Trusted edge, assets, limits, and shutdown](docs/standalone-deployment.md), [idempotency](docs/standalone-idempotency.md), [external guards](docs/standalone-guards.md) |
| Add authenticated host permission checks | [TypeScript standalone guard example](examples/standalone-typescript-guards/README.md); embedded Rust guards are demonstrated in the helpdesk below |
| Learn a larger SQLite scenario | [SQLite quickstart](examples/sqlite-quickstart/README.md): typed IDs, scopes, pagination, and soft deletion; requires a file database and migrations |
| Adapt to server databases | [MariaDB quickstart](examples/mariadb-quickstart/README.md), [PostgreSQL quickstart](examples/postgres-quickstart/README.md), [verified TLS](docs/database-tls.md) |
| Study an application architecture | [Advanced Axum helpdesk](examples/axum-helpdesk/README.md): multi-tenant PostgreSQL app with authentication, host guards, streaming, and optional Redis |
| Explore language examples | [Commerce schema](spec/examples/commerce/README.md): a broad reference schema rather than a first-run app |
| Evaluate measured costs | [Runtime measurements and limits](docs/runtime-performance.md), [compiler profiles](docs/compile-time.md), [consumer build-cost decision](docs/consumer-build-cost.md) |
| Change generator integration deliberately | [Optional Cargo-generation prototype](docs/cargo-generation.md), [external consumer contracts](docs/generated-consumer-contracts.md) |
| Prepare release artifacts | [Release dry runs and owner publication](docs/releasing.md), [candidate release notes](docs/release-notes.md) |

Tutorials explain a first successful run. The reference answers syntax questions;
the specification explains semantics and priorities. Focused recipes are being
prepared in [#60](https://github.com/oscscull/baseddsl/issues/60); advanced examples
currently provide larger worked scenarios. These serve different reading needs.

## Support and license

Current database CI covers bundled SQLite, MariaDB 11.4, and PostgreSQL 16.
MariaDB evidence does not establish MySQL server support. Read the
[support and compatibility policy](docs/support-policy.md) for native platform,
toolchain, deployment, and versioning boundaries. Performance links above report
specific workloads and tradeoffs; they do not establish a universal speed claim.

Based source, Rust crates, and the VS Code extension are
[AGPL-3.0-only](LICENSE). [Generated-output terms](LICENSE-GENERATED.md) cover Rust
clients, SQL, OpenAPI documents, and migration artifacts produced from your schemas.
Your schemas and data remain yours.

The implementation lives in [`crates/`](crates/); portable verification targets
live in the [`Makefile`](Makefile). Generator changes also run the fresh external
consumer gates described above.
