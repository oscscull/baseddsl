# PostgreSQL catalog reader

`based-catalog-postgres` implements the [shared import contract](import-catalog-contract.md)
behind an opt-in `postgres` feature. It owns PostgreSQL catalog queries and native
normalization, with no compiler/runtime-engine dependency, application-row query,
model writer, or migration operation. It does not depend on another reader.

The independently authored live fixture passes on **PostgreSQL 16.14**. CI uses
the existing PostgreSQL 16 service tier and records its exact version. Other server
majors/extensions are not an evaluated compatibility promise.

## Connection and visibility

The host supplies SQLx `PgConnectOptions` and production certificate-verified TLS
(`PgSslMode::VerifyFull`) with appropriate trust roots. The reader preserves those
options and disables statement logging; failures become fixed redacted
`CatalogReadError` values, not driver errors/URLs. The test fixture changes only
credentials, preserving TLS settings for the existing TLS-required CI tier.

The tested discovery account has schema **USAGE** and ordinary `pg_catalog`
metadata visibility. It has no application-table SELECT/mutation grant or sequence
privilege. Queries use direct catalogs rather than privilege-filtered
`information_schema` views that can hide facts from such an account. Missing schema
USAGE or denied catalog access is a metadata error; missing requested objects carry
blocking findings. Names are bound values, including unusual quoted identifiers.

Each discovery owns a **REPEATABLE READ READ ONLY** transaction, reads one catalog
snapshot, and rolls back; cancellation queues SQLx rollback. It never queries
application tables, evaluates defaults, calls nextval, inspects sequence state,
applies DDL, or provisions a migration baseline. Concurrent DDL can make the snapshot
older than the current schema: use a quiescent schema for subsequent generation and
review changes normally. Discovery does not continuously synchronize the database.

## Retained facts and limitations

Native declarations come from `format_type`; column catalog ordinals, nullability
(including a domain's NOT NULL rule), raw default/generated SQL and qualified
collations remain facts. Enum/domain identities remain namespace-qualified;
enum labels retain catalog order. Arrays/extensions are not flattened into text
or guessed from application rows. Numeric typmod decoding preserves negative
scales; timestamp/time precision and timezone variants stay distinct.

Identity ALWAYS/BY DEFAULT and sequence expressions remain distinct generation
strategies. Sequence options/state are not normalized: a named `DefinitionLoss`
warning requires retaining existing database generation and reviewing future
migration DDL. Domain rules/defaults remain opaque behind blocking `UnsupportedType`;
qualified identity is not a promise to recover a domain's complete definition.
No catalog success authorizes the emitter to skip exact native-type/default checks.

Primary/unique keys retain ordered parts and deferral. Declared FK pairs retain
qualified target identity, actions, match and timing; cycles are collected without
topological ordering or implicit table selection. CHECK expressions are retained.
Indexes retain ordered columns/expressions, included columns, predicates, qualified
collations, method, validity and native reconstructed definitions. Reconstruction
is catalog SQL, not original source formatting.

The shared/adapter screen blocks native enums/domains/arrays, generated columns,
CHECKs/deferred keys, special FK match/timing/actions, partial/expression/INCLUDE/
descending/invalid indexes, non-btree/operator-class/NULLS/storage/tablespace forms,
and unsupported objects. NULLS NOT DISTINCT, unvalidated/partial-delete-action or
exclusion/constraint-trigger semantics get named blocking findings. Native
partitioning/inheritance, table options, unlogged/temporary tables and row-security
flags require manual representation; they are never converted into application
policy. Clustering/replica-identity state receives a warning that it is not reproduced.
Views/materialized views retain their native query definition and are blocked for
automatic model emission. No full PostgreSQL DDL reproduction is claimed.

## Verification

The existing portable tier runs this proof as well as runtime/CLI contracts:

```sh
make ci-live-postgres POSTGRES_URL='postgres://postgres:TEST_PASSWORD@127.0.0.1:5432/based_test'
```

The focused command explicitly runs its live case; a missing URL fails rather than
skipping the requested proof:

```sh
TEST_POSTGRES_URL='postgres://postgres:TEST_PASSWORD@127.0.0.1:5432/based_test' \
  cargo test -p based-catalog-postgres --features postgres --test live -- --include-ignored --nocapture
```

Use a disposable administrator-controlled test database. The fixture creates/removes
three named schemas (`catalog fixture`, `catalog_other`, `catalog_hidden`) and one
named test role. Hand-written SQL covers quoted/unusual names, composite/absent keys,
null/literal defaults, cross-schema cycles/self references, native enum/domain/array/
identity/sequence/generated facts, negative scale, special indexes, FK timing/match/
actions, an unvalidated CHECK, row security, and a view. Shared assertions verify
canonical ordering, selection/outside references and loss findings. Missing schema
USAGE fails. SELECT, nextval, INSERT and ALTER are denied to the discovery account;
administrator before/after snapshots verify schema, seeded data and sequence state
remain unchanged. Connection error reports contain no credentials/database names.
The ordinary workspace gate does not claim live proof; the service tier runs it.

The queries follow PostgreSQL 16's [pg_attribute](https://www.postgresql.org/docs/16/catalog-pg-attribute.html),
[pg_type](https://www.postgresql.org/docs/16/catalog-pg-type.html),
[pg_constraint](https://www.postgresql.org/docs/16/catalog-pg-constraint.html),
[pg_index](https://www.postgresql.org/docs/16/catalog-pg-index.html),
and [catalog reconstruction functions](https://www.postgresql.org/docs/16/functions-info.html).
Signed numeric scale decoding follows the [PostgreSQL 16 implementation](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/backend/utils/adt/numeric.c).
