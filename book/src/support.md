# Compatibility and reports

The current database matrix is bundled SQLite, MariaDB 11.4, and PostgreSQL 16.
MariaDB evidence does not establish MySQL support. Native host requirements are in
[installation](installation.md). Embedded Rust and standalone HTTP use the same engine.

Before v1, public interfaces can change between `0.x` versions. The v1 target
covers BSL behavior, generated Rust, documented runtime APIs, HTTP routes/formats,
and migration formats. Regenerate clients with the matching compiler and preserve
applied migration history. Cursors are opaque; pass them back without decoding them.

Report bugs in [GitHub issues](https://github.com/oscscull/baseddsl/issues): include
the version, expected/actual behavior, and a small safe reproduction. Report
vulnerabilities through [private reporting](https://github.com/oscscull/baseddsl/security/advisories/new).
Support is best effort.

Source, runtime, and extension are AGPL-3.0-only. [Generated-output terms](../../LICENSE-GENERATED)
apply to generated clients, SQL, OpenAPI, and migration artifacts. Your schemas and
data remain yours.
