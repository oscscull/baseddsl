# Release notes

Each release has a section here; its archive and GitHub release carry that section.
Follow [upgrading](upgrading.md) for every version change.

## Unreleased

- A SQLite migration that rebuilds a table no longer breaks its child tables' foreign
  keys: apply turns foreign keys off around the rebuild and checks them before commit.

## 1.0.0

The first stable release: the surfaces in Compatibility and reports follow
semantic versioning from here. Upgrading from 0.1.x: regenerate every client.


- `list` takes a `limit` clause for a caller-chosen row cap.
- `Id::as_int` reads a `serial` id back as an integer.
- Timestamps and dates are validated at bind; SQLite stores one canonical form.
- Migrations embedded in the binary can be applied from the application.
- `get` and upsert conflict targets accept composite unique keys.
- Raw `{id}` expands to the model's real key column and `{table}` is schema-qualified;
  soft-delete raw overrides are applied instead of ignored.
- Field names in model decorators and `@index` resolve as LSP references.
- Every `#[allow]` in generated Rust states its reason.

## 0.1.13

First release candidate for the surfaces listed under Compatibility and reports.
Regenerate every client and review rendered migrations when upgrading.

- Embedded and standalone starters from `based init`; `build.rs` generation is the
  default Rust client path, with checked-in `generated/` output as the alternative.
- `based serve`: authenticated external HTTP guards, durable keyed-mutation storage,
  and a documented trusted-edge boundary.
- One-shot import of SQLite, MariaDB, and PostgreSQL schemas into checked BSL.
- Verified database TLS through Rustls.
- Typed migration data steps in `up.mig` (`format 2`) and a fully typed `down.mig`.
- `rel.any(pred)` predicates; to-many `where` paths lower to `EXISTS`.
- Composite foreign-key column aliases.
- Typed test fixtures for Rust and TypeScript.
- `based explain` and SQL statement logging.
- A `rusqlite` SQLite backend so embedders share one libsqlite3.
- Named filters project as boolean shape fields.
- Decimal fields keep the full 38-digit range through an exact generated type.
- Bundled SQLite floor 3.51.3 (WAL-reset corruption fix).
- Versioned CLI/LSP archives and a matching VS Code extension.
