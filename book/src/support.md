# Compatibility and reports

The current database matrix is bundled SQLite, MariaDB 11.4, and PostgreSQL 16.
MariaDB evidence does not establish MySQL support. Native host requirements are in
[installation](installation.md). Embedded Rust and standalone HTTP use the same engine.

Before v1, public interfaces can change between `0.x` versions; the
[release notes](release-notes.md) list each change. From v1, a breaking change to a
surface below needs a new major version.

| Surface | Stable from v1 | Not covered |
|---|---|---|
| BSL | A schema that checks keeps checking with the same meaning | New warnings; errors for constructs that were already silently wrong |
| Generated clients and fixtures | Names and types of generated operations, rows, and inputs for an unchanged schema | Generated file layout, private helpers, formatting |
| Runtime API | Documented `based_runtime` items used by generated clients and applications | Items not in the book or reference |
| HTTP service | Routes, request/response JSON, error codes, guard callback protocol | Log text, `based explain` output |
| Migrations | Snapshots and `format 1`/`format 2` migrations written by any v1 release apply, verify, and roll forward on later v1 releases | Rendered SQL text for a migration not yet applied |
| CLI | Command names, flags, and `based.toml` keys in the book | Human-readable output |

A generated client works only with the runtime version that generated it: upgrade
both together and regenerate. Applied migration history is never rewritten by an
upgrade. Cursors are opaque; pass them back without decoding them.

Report bugs in [GitHub issues](https://github.com/oscscull/baseddsl/issues): include
the version, expected/actual behavior, and a small safe reproduction. Report
vulnerabilities through [private reporting](https://github.com/oscscull/baseddsl/security/advisories/new).
Support is best effort.

Source, runtime, and extension are AGPL-3.0-only. [Generated-output terms](../../LICENSE-GENERATED)
apply to generated clients, SQL, OpenAPI, and migration artifacts. Your schemas and
data remain yours.
