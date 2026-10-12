# Upgrading

Keep CLI, language server, extension, runtime, and generated clients at matching
versions. Before v1, `0.x` releases can change public interfaces.

1. Back up database and migration history. Keep the previous tool installation.
2. Pin the new source/version for the runtime and `based-build`, then regenerate
   any checked-in artifacts with `based gen all`.
3. Compile the application, review API changes and rendered migration SQL, and test
   against a copy of the database.
4. Apply reviewed migrations separately and verify schema, data, and application calls.

Generated decimal fields use `bigdecimal`. An embedded client reaches it through
`based_runtime`; a wire client with decimal fields needs `bigdecimal = "0.4"`. Review
serializers and call sites when regenerating.

The bundled SQLite floor is 3.51.3 (libsqlite3-sys 0.37.0), which includes
the upstream WAL-reset corruption fix. A binary update does not repair existing
corruption; inspect affected databases and restore from a known backup when needed.
Consumer lockfiles and custom/system SQLite bindings must satisfy that floor too.

Rolling back tools does not roll back data or schema. See
[migration recovery](migrations.md) before retrying a failed migration or using down scripts.

## Known limitations

- Pre-v1: BSL, generated code, and HTTP formats can change between `0.x` releases.
- Tested on bundled SQLite, MariaDB 11.4, and PostgreSQL 16. MySQL is untested.
- `based serve` trusts `X-Based-Context` and has no identity provider; put it
  behind an authenticating edge ([deployment](deployment.md)).
- Idempotency records are never deleted automatically ([retrying mutations](idempotency.md)).
- MariaDB DDL is not transactional; a failed migration can leave partial changes
  ([migrations](migrations.md)). Migrations do not provide zero-downtime changes.

## Licensing

Based's source, runtime, and VS Code extension are AGPL-3.0-only ([LICENSE](../../LICENSE)).
Generated clients, SQL, OpenAPI documents, and migrations fall under the
[generated-output permission](../../LICENSE-GENERATED): you choose their terms.
Your schemas and data remain yours.
