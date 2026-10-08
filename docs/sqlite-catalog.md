# SQLite catalog reader

`based-catalog-sqlite` implements the [shared import contract](import-catalog-contract.md)
behind its opt-in `sqlite` feature. It owns SQLite connection and catalog
normalization; it has no compiler, runtime-engine, model writer, or migration
dependency. The independently authored fixture records **SQLite 3.51.3**
from the locked SQLx bundled engine. The [readiness review](v1-readiness.md)
requires its upstream WAL-reset fix in both the reader and runtime. Metadata success does not establish that
existing application rows can decode into generated Rust types.

## File and read boundary

`SqliteCatalogReader::open` accepts an existing file path. It opens with SQLx
read-only mode, creation disabled, and `query_only=ON`. It never uses `immutable`
for a live database: committed schema in an existing WAL must remain visible.
Selection uses exact names in the `main` namespace; no ATTACH, wildcard, or
implicit dependency selection is supported. Metadata names are bound values.
Each discovery owns a transaction and rolls back after reading one snapshot.
Cancellation queues rollback. Use a quiescent schema for later generation.

SQLite's authorizer also denies application-table reads, including `count(*)`.
Only reserved schema catalogs and the five required built-in PRAGMA table
functions may be read. Before installing it, a schema-only check rejects objects
whose names could shadow those functions. Views and virtual/shadow tables retain
their kind and native definition with blocking findings; their definitions are
not resolved by querying their rows. The small FFI boundary holds SQLx's exclusive
handle lock when installing a static callback, retains no borrowed state, and
borrows SQLite's checked non-NULL callback strings only during that callback.

The reader never evaluates defaults, reads application rows, executes DDL,
provisions a ledger, or writes/checkpoints the database. Read-only WAL access can
participate in SQLite shared-memory coordination; it is not a promise that every
sidecar byte stays unchanged or that the enclosing filesystem is unwritable.
Tests separately prove unchanged main database and existing WAL bytes. Opening a
missing path fails without creating it. Operational errors are fixed redacted
`CatalogReadError` values, and source identity contains only the file basename,
engine family, and version.

## Retained facts and limitations

Native declarations and SQLite's ordered affinity rules are preserved. An empty
declaration is valid BLOB affinity with unknown native type, not missing metadata;
automatic conversion remains blocked. NUMERIC affinity is not evidence of a
boolean, decimal, UUID, or date. STRICT ANY retains its special absence of type
coercion and remains unsupported. Nullability, raw default SQL, column ordinals,
STRICT/WITHOUT ROWID, and ordered primary/unique keys remain separate facts.

An actual single-column INTEGER PRIMARY KEY rowid alias is distinguished from an
indexed integer key. AUTOINCREMENT and ordinary rowid reuse remain different
generation strategies. The historical INTEGER PRIMARY KEY DESC exception is
preserved. WITHOUT ROWID and STRICT keys receive their enforced non-null facts;
nullable ordinary SQLite primary keys produce a blocking finding.

Declared FKs preserve paired columns, actions, self references, and cycles. A
REFERENCES clause without target columns resolves the target's declared PK from
metadata, including outside the selected set; it never adds that table to the
result. SQLite enforces SIMPLE matching. Declared MATCH syntax is retained in
native DDL with a blocking finding. PRAGMAs cannot expose per-FK deferral: when
timing syntax is present, timing is explicitly `Unknown` and blocks conversion.

Indexes retain ordered parts, direction, collation, origin, uniqueness, and native
definition; auxiliary storage parts are not invented INCLUDE columns. Expression
parts, partial predicates, and generated expressions are unavailable individually
and retain their native definitions with blocking `IncompleteMetadata` findings.
CHECK, explicit column COLLATE, and ON CONFLICT clauses likewise require manual
representation. Default BINARY column collation is asserted only when no COLLATE
declaration occurs. Named constraint identifiers remain in native DDL with a
definition-loss warning where normalized PRAGMA keys/FKs cannot expose their names.
The keyword scanner ignores quoted identifiers, strings, and comments; it does
not claim to parse arbitrary native expressions. No full SQLite DDL reproduction
or stronger application policy is inferred.

## Verification

The existing fast SQLite workspace tier includes this reader:

```sh
make ci-workspace
cargo test -p based-catalog-sqlite --features sqlite -- --nocapture
```

Six focused tests use temporary independently authored databases. They cover
quoted names, composite/absent keys, nullable/literal defaults, declared self/cyclic
FKs, implicit referenced PKs, rowid/AUTOINCREMENT/DESC, STRICT and untyped columns,
generated expressions, expression/partial indexes, and unsupported views. Shared
assertions verify canonical ordering, closed selection, outside references, and
loss findings. Tests on the reader's actual connection deny SELECT, count(*),
INSERT, ALTER, and temporary-table creation; they also reject PRAGMA-name
shadowing. Before/after file comparisons prove unchanged seeded databases and
unchanged committed WAL content while a separate fixture writer remains open.

The normalization follows SQLite's [PRAGMA catalog interfaces](https://www.sqlite.org/pragma.html),
[affinity rules](https://www.sqlite.org/datatype3.html),
[rowid and key semantics](https://www.sqlite.org/lang_createtable.html),
[STRICT tables](https://www.sqlite.org/stricttables.html), and
[authorizer API](https://www.sqlite.org/c3ref/set_authorizer.html).
