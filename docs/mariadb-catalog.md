# MariaDB catalog reader

`based-catalog-mariadb` implements the [shared import contract](import-catalog-contract.md)
for MariaDB metadata behind its opt-in `mariadb` feature. It owns connection setup, catalog queries, native normalization,
and adapter findings. It has no runtime-engine/compiler dependency, model writer,
application-row query, or migration operation. Other readers do not depend on it.

The independently authored live fixture passes on **MariaDB 11.4.12 / InnoDB**.
CI uses the existing MariaDB 11.4 service tier and records its exact server version.
The adapter checks the server identifies itself as MariaDB; MySQL is not evaluated.
Non-InnoDB tables and unverified native table options receive blocking findings.

## Connection and visibility

The host supplies SQLx `MySqlConnectOptions`; the reader honors those options and
disables statement logging. Production hosts must select certificate-verified TLS
(`MySqlSslMode::VerifyIdentity`) and the correct trust roots. The live fixture preserves the administrator URL's TLS settings when replacing
its credentials, so the same proof also runs against the TLS-required CI server. Connection/query failures become fixed redacted
`CatalogReadError` values; the reader exposes no driver error or connection URL.

Grant the discovery account **REFERENCES** on each selected whole table or on its
exact database. This gives tested metadata visibility without application-row
SELECT or mutation privileges. SHOW VIEW can expose view metadata, but a definition
hidden without SELECT is reported as `IncompleteMetadata`; views are unsupported
for automatic model emission regardless. A SELECT grant can also establish complete
metadata visibility, but least-privilege REFERENCES-only credentials are the proof
used by this reader's tests. It never uses SELECT against application tables.

The visibility check requires a direct whole-table, exact-database, or global
REFERENCES/SELECT grant for the authenticated account. Column-only grants cannot
pass as complete catalogs. Role-only or wildcard-database grants are not evaluated
by this check and may be rejected; use the direct exact-object grant above.
A missing/hidden requested object cannot produce successful automatic import.

Each discovery owns a `START TRANSACTION READ ONLY` SQLx transaction and rolls it
back, including queued rollback on cancellation. Metadata names use bound values;
SHOW CREATE TABLE identifiers use doubled-backtick quoting. Two full canonical
passes must agree or discovery returns `InconsistentSnapshot`. MariaDB catalog DDL
is not an MVCC snapshot: run discovery against a quiescent schema. Two matching
passes are change detection, not a guarantee against concurrent DDL changing and
changing back between observations.

## Retained facts and limitations

The adapter reads ordered native columns/default SQL/nullability/charset/collation,
primary and unique keys, ordered indexes, declared FK pairs and actions, CHECK
expressions, and native table definitions. It preserves auto-increment, unsigned
ranges and generated expressions/storage. It does not interpret tinyint as boolean,
text as an application enum/JSON type, or `_id` names as relations. MariaDB JSON
aliases retain their actual native declaration and CHECK constraints.

Unknown/native enum types, generated/unsigned fields, prefix/ignored/special-method
indexes, native table options and extra column attributes (such as ON UPDATE or
INVISIBLE) carry blocking shared/adapter findings. Enum labels remain in the full
native declaration rather than an invented parsed label list; `IncompleteMetadata`
makes that normalization limitation explicit. Raw definitions retain details the
normalized contract cannot express. No loss finding promises exact emitted DDL.
The BSL emitter and CLI remain separate issues.

## Verification

Run the same existing live tier as CI:

```sh
make ci-live-mariadb MARIADB_URL='mysql://root:TEST_PASSWORD@127.0.0.1:3306/based_test'
```

The focused command requires infrastructure and runs its explicitly ignored live
case; absence of `TEST_MARIADB_URL` fails rather than skipping this proof:

```sh
TEST_MARIADB_URL='mysql://root:TEST_PASSWORD@127.0.0.1:3306/based_test' \
  cargo test -p based-catalog-mariadb --features mariadb --test live -- --include-ignored --nocapture
```

Use a disposable administrator-controlled test server: the fixture creates and
removes its own `based_catalog_fixture` database and two named test users. Its SQL
covers composite/absent keys, backticks/spaces/hyphens, null/literal defaults,
self references/cycles, native unsupported attributes and a view. Shared assertions
verify canonical ordering, closed selection, outside references, missing tables and
loss findings. A column-only account must fail. The REFERENCES-only account cannot
SELECT application rows, INSERT, or ALTER; administrator snapshots verify schema
and seeded data remain unchanged after discovery. Operational connection errors
are tested without credentials/database names in the report. Ordinary workspace
tests do not claim live proof; `ci-live-mariadb` runs it on the service.

The query sources follow MariaDB's [COLUMNS](https://mariadb.com/docs/server/reference/system-tables/information-schema/information-schema-tables/information-schema-columns-table),
[STATISTICS](https://mariadb.com/docs/server/reference/system-tables/information-schema/information-schema-tables/information-schema-statistics-table),
[KEY_COLUMN_USAGE](https://mariadb.com/docs/server/reference/system-tables/information-schema/information-schema-tables/information-schema-key_column_usage-table),
and [REFERENTIAL_CONSTRAINTS](https://mariadb.com/docs/server/reference/system-tables/information-schema/information-schema-tables/information-schema-referential_constraints-table)
metadata interfaces. Visibility uses the [grant tables](https://mariadb.com/docs/server/reference/sql-statements/account-management-sql-statements/grant)
with the concrete REFERENCES-only behavior verified by the live fixture.
