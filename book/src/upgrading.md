# Upgrading

Keep CLI, language server, extension, runtime, and generated clients at matching
versions. Before v1, `0.x` releases can change public interfaces.

1. Back up database and migration history. Keep the previous tool installation.
2. Pin the new source/version and regenerate clients and other owned artifacts.
3. Compile the application, review API changes and rendered migration SQL, and test
   against a copy of the database.
4. Apply reviewed migrations separately and verify schema, data, and application calls.

Generated decimal fields use `bigdecimal`; the consuming crate needs
`bigdecimal = "0.4"`. Review serializers and call sites when regenerating.

The bundled SQLite floor is 3.51.3 (libsqlite3-sys 0.37.0), which includes
the upstream WAL-reset corruption fix. A binary update does not repair existing
corruption; inspect affected databases and restore from a known backup when needed.
Consumer lockfiles and custom/system SQLite bindings must satisfy that floor too.

Rolling back tools does not roll back data or schema. See
[migration recovery](migrations.md) before retrying a failed migration or using down scripts.
