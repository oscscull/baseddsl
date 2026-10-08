# V1 support and compatibility policy

Based aims directly at v1 with two equally important deployment paths: an
embedded Rust engine with a generated typed async client, and a standalone
service called over HTTP. This is a target contract, not a claim that v1 is
ready. The [v1 backlog](https://github.com/oscscull/baseddsl/issues/43)
tracks the remaining release gates.

## Current evidence and v1 target

The [CI workflow](../.github/workflows/ci.yml) runs the
[portable Makefile gates](../Makefile). The matrix distinguishes configurations
tested in CI from configurations that currently have only implementation or
local evidence. The [successful main CI run at a8871be](https://github.com/oscscull/baseddsl/actions/runs/37337539542)
passed all six jobs. Server image tags identify the tested release series, not
every patch or older version within that series.

| Surface | Current automated evidence | V1 support boundary |
| --- | --- | --- |
| Rust toolchain | CI uses `stable`; workspace metadata declares 1.85, but the current lockfile's SQLx 0.9 requires 1.94. The minimum toolchain is not separately built in CI. | Use current stable for source installation. Rust 1.85 is not a supported full-runtime floor today; reconcile metadata and verify a minimum toolchain in the [distribution gate](https://github.com/oscscull/baseddsl/issues/56). |
| Host platform | GitHub Actions runs on `ubuntu-latest` for all jobs. | Linux is CI verified. macOS and Windows are not currently CI verified. |
| SQLite | Workspace tests, image smoke, and a file-backed quickstart in `ci-examples`; SQLx 0.9 bundles SQLite through the locked `libsqlite3-sys` dependency. No external server. | Support the bundled engine selected by the release lockfile; arbitrary system SQLite versions are unverified. Embedded Rust and standalone HTTP are both release paths. |
| MariaDB 11.4 | Dedicated live integration job and quickstart on `mariadb:11.4`. | Explicit MariaDB support; its SQL features and tests are distinct from MySQL. |
| PostgreSQL 16 | Dedicated live integration job and quickstart on `postgres:16`. | Explicit PostgreSQL support. |
| Database TLS | Dedicated `database-tls` gate provisions test-CA Postgres 16 and MariaDB 11.4 fixtures, runs CLI/embedded quickstarts and live suites, and rejects untrusted CA/wrong hostname. | Explicit certificate and hostname verification through SQLx with Rustls; see [configuration](database-tls.md). SQLite-only consumers remain TLS-free. |
| MySQL | Code paths and some tests exist, but CI provisions MariaDB, not MySQL. | Do not infer MySQL server support from MariaDB CI; add MySQL server evidence before listing a version as supported. |
| Embedded Rust | Generated client, runtime tests, and three database quickstarts in `ci-examples`. | First-class typed async Rust output. The application supplies its database connection, context, and host guard functions. |
| Standalone HTTP | `ci-image` boots the server with SQLite; HTTP and runtime tests run in `ci-workspace`. | First-class service path; release verification must cover create/read and authenticated guard callbacks on the supported databases. Named external guard callbacks use [operator-configured authenticated HTTP endpoints](standalone-guards.md). |
| VS Code extension | `ci-extension` compiles and packages the extension with Node 20. Its manifest accepts VS Code `^1.75.0`; CI does not launch every VS Code version. | Editor installation and matching LSP distribution need the separate [#57](https://github.com/oscscull/baseddsl/issues/57) gate. |

The repository examples show current source-based setup:
[embedded SQLite](../examples/sqlite-quickstart/README.md),
[embedded MariaDB](../examples/mariadb-quickstart/README.md), and
[embedded PostgreSQL](../examples/postgres-quickstart/README.md). The CLI's
`based serve` command is the current standalone entry point. Versioned
installation and a complete standalone tutorial are tracked by
[#56](https://github.com/oscscull/baseddsl/issues/56) and
[#70](https://github.com/oscscull/baseddsl/issues/70). Both paths must pass
their documented onboarding and release checks before v1 is declared ready.
For source installation today, use current stable and run from this checkout:

```sh
cargo install --locked --path crates/based-cli
cargo install --locked --path crates/based-lsp
```

For embedded execution, follow a database quickstart and its runtime feature and
client dependency declarations. For standalone execution, use `based serve --help`
and the [container setup](../docker/README.md); the trusted auth edge must strip
caller-supplied `X-Based-Context` and replace it with authenticated context, and
direct access to the listener must be restricted. These source/container paths
are available today; published package installation and external onboarding
remain release gates for both modes, not assumed capabilities.

## Execution and guards

Both paths execute compiled BSL through the Based engine and database driver.
The generated embedded client converts typed Rust arguments and context to JSON
values, calls the in-process engine, and decodes JSON values back to Rust types.
It avoids an HTTP socket, but it still uses the engine and this JSON conversion;
it is not native Rust execution of a compiled query. The standalone listener
uses the same engine behind HTTP and the documented wire format. No overhead
comparison is asserted until [#55](https://github.com/oscscull/baseddsl/issues/55)
measures it.

A declared guard names an application decision that must run before its
mutation. In embedded Rust, the application registers an async closure by name
through `Guards`; engine construction rejects missing registrations. The
standalone v1 contract requires authenticated, named HTTP callbacks supplied
by a trusted backend, with missing mappings rejected at startup and callback
failures denying the write. The callback contract is implemented by the optional
`external-guards` adapter, enabled by the CLI. Callbacks decide
authorization; database write conditions must still enforce atomic invariants.
The application or trusted edge derives `$ctx` and protects callback credentials.

## Compatibility boundaries

Before v1, `0.x` releases may change these interfaces; migration files and
generated code should be reviewed when upgrading. For v1, incompatible changes
to the following public surfaces require a major version and migration notes:

| Surface | V1 compatibility commitment |
| --- | --- |
| BSL | Parsed syntax, type rules, and observable model/query/mutation behavior. |
| Generated Rust | Documented client types, method signatures, and serialization behavior. Regenerate clients with the matching compiler version. |
| Runtime | Documented public `based-runtime` APIs used by embedded applications. Internal compiler modules have no stability promise. |
| HTTP | Routes, request and response JSON, stable error codes, and cursor behavior. A cursor is opaque and should be passed back, not decoded by callers. |
| Migrations | Stored `.mig` and `.snap` formats and their application semantics. Applied migrations remain immutable; generators do not silently rewrite history. |

The v1 target does not include new client languages, a hosted platform, a broad
syntax redesign, or replacement database drivers. Async support is part of the
Rust API, not a claim that other tools lack it.
