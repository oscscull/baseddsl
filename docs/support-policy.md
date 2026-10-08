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
local evidence. Review all triggered checks on the candidate's exact PR head,
including the [native release dry run](../.github/workflows/release.yml); a historical
green run does not verify later changes. Server image tags identify the tested
release series, not every patch or older version within that series.

| Surface | Current automated evidence | V1 support boundary |
| --- | --- | --- |
| Rust toolchain | Workspace metadata requires 1.94, matching SQLx 0.9. The release source job builds CLI/LSP and a fresh pinned-Git library consumer on 1.94; other CI jobs use stable. | Rust 1.94+ with platform C tools for source builds; see [versioned installation](installation.md). |
| Host platform | Native release dry runs build and execute extracted CLI/LSP/SQLite migration smoke on Linux x86_64/arm64, macOS arm64/Intel, and Windows x86_64. | The exact OS/glibc/native architecture boundaries are in the [installation matrix](installation.md); other hosts have no prebuilt-binary support promise. |
| SQLite | Workspace tests, image smoke, and a file-backed quickstart in `ci-examples`; SQLx 0.9 bundles SQLite through the locked `libsqlite3-sys` dependency. No external server. | Support the bundled engine selected by the release lockfile; arbitrary system SQLite versions are unverified. Embedded Rust and standalone HTTP are both release paths. |
| MariaDB 11.4 | Dedicated live integration job and quickstart on `mariadb:11.4`. | Explicit MariaDB support; its SQL features and tests are distinct from MySQL. |
| PostgreSQL 16 | Dedicated live integration job and quickstart on `postgres:16`. | Explicit PostgreSQL support. |
| Database TLS | Dedicated `database-tls` gate provisions test-CA Postgres 16 and MariaDB 11.4 fixtures, runs CLI/embedded quickstarts and live suites, and rejects untrusted CA/wrong hostname. | Explicit certificate and hostname verification through SQLx with Rustls; see [configuration](database-tls.md). SQLite-only consumers remain TLS-free. |
| MySQL | Code paths and some tests exist, but CI provisions MariaDB, not MySQL. | Do not infer MySQL server support from MariaDB CI; add MySQL server evidence before listing a version as supported. |
| Embedded Rust | Generated client, runtime tests, and three database quickstarts in `ci-examples`. | First-class typed async Rust output. The application supplies its database connection, context, and host guard functions. |
| Standalone HTTP | `ci-image` boots the server with SQLite; HTTP and runtime tests run in `ci-workspace`. | First-class service path; release verification must cover create/read and authenticated guard callbacks on the supported databases. Named external guard callbacks use [operator-configured authenticated HTTP endpoints](standalone-guards.md). |
| VS Code extension | `ci-extension` packages the extension; the native release `vsix` job runs diagnostics, hover, completion and rename in an isolated VS Code profile with the matching LSP. Its manifest accepts VS Code `^1.75.0`; CI does not launch every VS Code version. | Install the matching versioned extension/LSP from the [distribution matrix](installation.md); there is no Marketplace or all-editor-version promise. |

The repository examples show current source-based setup:
[embedded SQLite](../examples/sqlite-quickstart/README.md),
[embedded MariaDB](../examples/mariadb-quickstart/README.md), and
[embedded PostgreSQL](../examples/postgres-quickstart/README.md). The CLI's
`based serve` command is the current standalone entry point. Versioned
[installation and pinned source/library routes](installation.md) have a
[non-publishing dry-run workflow](releasing.md). The
[embedded](embedded-tutorial.md) and [standalone](standalone-tutorial.md) tutorials
are replayed by the [onboarding gate](onboarding-gate.md). Both paths
must pass their documented onboarding and owner release checks before v1 is
declared ready. Public release assets are not assumed to exist before those gates.

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
uses the same engine behind HTTP and the documented wire format. The [matched runtime benchmark](runtime-performance.md) reports measured costs
and their limits; it does not establish a universal runtime overhead ratio.

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

## Reports and maintainer commitment

Ordinary bugs use [GitHub issues](https://github.com/oscscull/baseddsl/issues/new/choose).
Provide the version, relevant model/query or typed call, expected/actual behavior
and a minimal safe reproduction. Use fabricated data; remove credentials, tokens,
private rows and unapproved source. Security-sensitive behavior goes through the
[enabled private reporting route](../SECURITY.md).

Support and review are best effort. There is no response-time SLA, paid support
promise or commitment to implement every feature request or support every historical
version. Recurring, reproducible barriers to useful work are actionable evidence;
reporters need not understand compiler internals. See [contribution guidance](../CONTRIBUTING.md)
for SRP and the focused/full verification tiers.

[One-shot database import](import-existing-database.md) preserves external migration
ownership; it does not provide continuous sync, a merge engine or implicit baseline.
Generated artifacts and stored migration history retain their documented ownership
and compatibility boundaries.
