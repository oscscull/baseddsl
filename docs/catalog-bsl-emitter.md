# Catalog-to-BSL model emission

`based-catalog-bsl` consumes the [shared physical catalog](import-catalog-contract.md)
and explicit selection with the project's existing manifest. It performs no SQL,
filesystem reads/writes, migration baselining, or application-data inspection.
Connection/discovery remains in the independent dialect readers; exclusive model
publication belongs to the later CLI integration in [#67](https://github.com/oscscull/baseddsl/issues/67).

The result is one canonically formatted model file per selected table, relative to
the configured schema root. All models pass `based-fmt` and the same
`based-project::check_sources` pipeline used by ordinary project checking. The
emitter then independently compares compiler-resolved physical table identities,
column names/nullability, ordered primary keys, uniqueness/index parts, and declared
FK pairs/targets/actions with the catalog. A failure returns an import report and
**no publishable model files**. Compiler warnings remain visible, including unused
or missing-index guidance; they never create indexes or change the database.

## Naming and relationships

One registry covers the whole selected set, in canonical physical identity order.
ASCII words become PascalCase model names and snake_case fields; camel-case word
boundaries split, punctuation separates words, and non-ASCII code points become
`uHEX` words. Empty/digit-leading/reserved names receive `Imported`/`imported_`
prefixes. Model collisions receive numeric suffixes (`OrderItem2`), since BSL's
model lexer does not accept underscores; fields receive `_2`, `_3`, and so on.
Model filenames are allocated case-insensitively for portable publication.
Every table and column retains an explicit physical alias. PostgreSQL/MariaDB
namespaces retain `@schema`; SQLite selects only its existing `main` file.
Dotted table aliases or whitespace namespaces fail rather than rename a DB object.

Required natural/composite PK parts become an ordered `@key`. A genuinely keyless
table receives `@no_id("imported keyless table")`; names such as `id`, `owner_id`,
or `created_at` never fabricate a key, relation, scope, or lifecycle annotation.
Representable sole generated keys become `id: serial` with their original column
alias. No scopes, soft deletion, inverse edges, queries or mutations are inferred.

Only declared single-column FKs targeting the selected target's actual primary
key become forward fields. Their physical aliases, nullability and actions are
checked after compiler resolution. A reason records declared FK presence when
the project convention is `none`. Self references and cycles need no topological
order. Outside references, alternate unique-key targets, overlapping FK columns,
relation defaults/generation, and arbitrary composite FK aliases block emission.
BSL's existing composite-key expansion cannot express arbitrary per-part aliases;
the catalog keeps every pair for manual work. Explicit per-part support is tracked
in [#123](https://github.com/oscscull/baseddsl/issues/123), beyond the initial matrix.
See the existing
[key syntax](../spec/syntax/models.md) and [relation guarantees](../spec/syntax/relations.md).

## Verified mappings and explicit losses

The dialect mapping is deliberately closed. SQLite maps verified integer, real,
text and blob declarations without inventing boolean/date/UUID/decimal semantics
from affinity or values. PostgreSQL covers signed integers, floating types,
boolean, text, bytea, UUID, JSONB, date, timezone-aware timestamp, timezone-free time,
and bounded numeric fitting `1 ≤ scale ≤ precision ≤ 38`. MariaDB covers signed
integer/float, text/binary families, native date/DATETIME and compatible decimal.
MariaDB native UUID, duration-valued TIME, PostgreSQL timezone-free timestamp,
timezone-aware time, JSON rather than JSONB, unbounded/negative-scale numerics,
and unknown types require manual representation. Catalog enum/domain/array,
unsigned/generated, CHECK, deferred and special-index findings still block.
Opaque `raw` types are not used to claim stronger automatic decode/write guarantees.

Native width, precision, character set/collation, constraint/index names, unique
constraint-versus-index DDL form, and SQLite STRICT/WITHOUT ROWID properties that
BSL does not reproduce receive physical-object `DefinitionLoss` warnings.
Those warnings require review before any future migration DDL; emission does not
alter existing DB behavior or authorize adopting its migration history. Index
collations differing from their columns and non-btree access methods are blocked.
Constraint-backed indexes are represented through their matching declared keys,
not emitted twice. The original catalog remains the native-definition authority.

`serial` is verified for SQLite INTEGER PRIMARY KEY AUTOINCREMENT, MariaDB signed
BIGINT AUTO_INCREMENT, and PostgreSQL BIGINT GENERATED ALWAYS AS IDENTITY sole
keys. Rowid reuse, BY DEFAULT identity, native sequences, non-key/composite
generation and generated expressions are rejected instead of changing their strategy.

Defaults preserve verified nonnegative integer, fixed decimal/float, boolean,
NULL, and SQL single-quoted text literals, including doubled quotes. PostgreSQL
text/character-varying literal casts have an explicit checked mapping. MariaDB
backslash-dependent literals are rejected because their interpretation depends
on SQL mode. BSL's current default grammar does not accept signed numeric literals;
these get a named blocking finding, with future syntax support tracked in
[#122](https://github.com/oscscull/baseddsl/issues/122). Temporal/UUID/JSON/binary defaults, arbitrary
casts, functions and other SQL expressions require manual authoring. No default
is evaluated and no application-row decode compatibility is inferred.

## Verification

```sh
cargo test -p based-catalog-bsl
cargo clippy -p based-catalog-bsl --all-features --all-targets -- -D warnings
make ci-workspace
```

Independent catalog fixtures assert actual resolved primitive/default values,
physical aliases, ordered natural/composite/absent keys, declared FK pairs/actions,
cycles/self references, indexes, reserved/Unicode/collision behavior, and repeatable
output under reversed catalog arrival. Rejection tests cover outside selection,
native generation/time/numeric variants, unknown/signed defaults, unrepresentable
aliases, opaque/special-index semantics and nullable PK compiler failures.
These are pure emitter proofs; the independently authored SQL fixtures and
read-only credentials/file proofs remain in each reader's existing database tier.
The CLI issue supplies the real typed query against the original unchanged DB.
