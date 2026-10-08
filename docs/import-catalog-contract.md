# Read-only database import contract

The first importer reads MariaDB, PostgreSQL, and SQLite catalogs. It begins only
after the blank-project [onboarding gate](onboarding-gate.md) passes. Discovery
feeds the existing configured model/generation workflow; it is not another
initializer, migration baseline, or application-data inspector.

[`based-catalog`](../crates/based-catalog/src/lib.rs) owns physical facts,
selection, diagnostics, deterministic ordering, and the asynchronous
`CatalogReader` seam. It depends on no SQLx/runtime/compiler crate and performs
no database I/O. Each adapter owns its connection, dialect catalog SQL, and
native normalization. The later emitter consumes facts without querying a DB.

## Facts and read boundary

`CatalogSource` records family, server version, and logical database identity;
it contains no URL or credentials. Tables retain namespace/name identity, kind,
ordered columns, native declarations/modifiers/character sets, nullability, raw defaults,
collations and generation facts; primary/unique keys, index definitions, declared
FK pairs/actions/match/deferral, and CHECK expressions remain separate records.
Native enums/domains retain qualified identity; SQLite retains declared affinity,
rowid/autoincrement, STRICT and WITHOUT ROWID facts. An unavailable individual
expression can be marked absent with its original table/index definition retained;
it must carry a blocking incomplete/unsupported finding, never a guessed value.

Readers use a genuinely read-only connection/transaction and metadata queries
only. They never sample application rows, mutate schema/data, initialize a
ledger, run generated DDL, or infer scopes, soft deletion, lifecycle timestamps,
UUID meaning, relations from `_id` names, or business constraints. A context-free
physical integer/text field is not evidence of application intent.

Metadata visibility must cover every explicitly selected object and its requested
facts. An unavailable table, hidden constraints, failed catalog query, or changed
schema is an error; a partial result cannot be represented as complete success.
Operational errors use fixed `CatalogReadError` variants, which cannot retain a
raw connection URL or driver error. Adapter diagnostics use physical object IDs
and safe messages, never SQLx connection/debug payloads.

## Selection, references, and ordering

Selection is a nonempty closed set of exact physical namespace/table identities.
Names are not interpolated into metadata SQL. Readers bind values or use the
driver's identifier quoting for catalog interfaces that require identifiers.
No wildcard selection or implicit dependency expansion is part of this seam.
Missing requested tables and unexpected returned tables are blocking findings.

A declared FK to an unselected table remains a full fact with ordered local and
target column pairs, plus `OutsideSelection`. The model emitter must refuse that
dependency until the operator explicitly selects the target. It must not invent
a relation, silently replace it with an unrelated scalar, or include extra tables.
Self references and cycles are valid discovery results: collect all selected
facts before resolving references; do not demand a topological table order.

Canonical order is namespace/name for tables, catalog ordinal for columns,
name/column list for unique keys, physical name for indexes, name/target/column
pairs for FKs, and name/expression for checks. Diagnostics sort/deduplicate by
their full records. **Do not sort key parts, FK pairs, index parts/included
columns, or enum labels alphabetically**: their order has semantic meaning.
Duplicate identities, duplicate column ordinals, missing key/index columns, and
dangling selected references fail structural validation.

## Deterministic model naming policy

The later emitter builds one naming registry over the complete selected set.
It uses PascalCase model names and snake_case field names derived from physical
names; punctuation splits words, non-ASCII code points get deterministic `uHEX`
tokens, and empty/digit-leading identifiers receive `Imported`/`imported_`
prefixes. Reserved BSL identifiers receive the same prefixes. Resolve normalized
collisions by `_2`, `_3`, … in canonical physical identity order, never query
arrival order. Field collision suffixes use catalog column order.

Always preserve the original identity through `@table`, `@schema`, and field
`(column "…")` aliases as necessary. An alias the actual compiler cannot
represent (for example a dotted table name or a whitespace namespace) is a
blocking diagnostic, not a renamed physical table. Preserve physical PK and FK
column order. Only declared FKs can become forward relations; add no inverses,
convenience callables, scope, or soft-delete policy automatically.

One model goes in one file under the configured schema root. The emitter checks
the complete output with the normal compiler and validates physical key/relation
mapping before publication. Composite FK mappings that cannot preserve every
physical column must fail explicitly. Existing files and applied migration
history are not overwritten or adopted by discovery.

## Initial capability and loss matrix

Catalog reading preserves facts even where automatic model emission is rejected.
`Discovery::checked` applies shared structural validation and a conservative
capability screen, plus adapter findings. `has_errors()` blocks automatic
continuation; absence of a discovery error is not permission to skip the
emitter's exact type/default/alias checks. Warnings identify a retained fact whose
model/DDL fidelity needs review. Every omitted or changed semantic property must
have a named finding; no silent best-effort output is a success.

| Catalog feature | Initial contract |
| --- | --- |
| Ordinary selected base tables | Read all metadata; emitter uses physical aliases and existing configured workflow |
| Scalar native types | Preserve full native declaration and modifiers. Emit a BSL primitive only when the dialect mapping is verified; never infer stronger types from rows or names |
| Unknown, array, native enum/domain types | Retain declaration/qualified identity/labels; shared blocking `UnsupportedType`. Manual BSL raw/enum authoring may be possible, but is not an automatic conversion promise |
| Unsigned numeric range | Retain unsigned flag; block silent mapping to signed `int` |
| Narrow integer/float, bounded text, native time/JSON variants | Preserve native details; exact emitter mapping must report bounds/semantic differences or reject them. No claim of exact DDL reproduction |
| Collations | Preserve effective column/index collation. BSL has no general column-collation modifier; emitter must report the definition loss or require manual representation. Existing DB semantics are not changed by discovery |
| Literal defaults | Preserve original SQL; emitter must decode only supported dialect literals and retain exact numeric spelling |
| Default expressions | Preserve SQL; only explicitly verified mappings may be emitted. Unknown functions/casts must be reported/rejected, not discarded or inferred as lifecycle annotations |
| Auto-increment, identity, sequence, SQLite rowid | Preserve strategy details separately from default text. `serial` is allowed only where its actual backend behavior matches; SQLite rowid reuse and PostgreSQL sequence/identity differences must be reported |
| Generated expressions | Retain expression/storage or full native definition; shared blocking finding until manually represented. BSL supports a restricted computed expression language, not arbitrary SQL recovery |
| Simple ordered-column indexes/unique keys | Preserve parts and uniqueness; PK/constraint-backed indexes are distinguished from explicit indexes so they are not emitted twice |
| Expression/partial/prefix/descending/INCLUDE/invalid indexes | Retain definition and part/predicate facts; shared blocking finding. Manual `@index raw` can represent many forms, but blind rewriting is excluded |
| Views/materialized views/virtual tables | Preserve kind and available metadata; shared blocking `UnsupportedObject`, never pretend they are mutable base tables |
| CHECK expressions, deferrable keys | Preserve and block automatic loss; manual schema work is required |
| Keyless tables | Preserve absence of PK; later output uses explicit `@no_id("imported keyless table")`, with documented keyed-operation limits, never a fabricated id |
| Natural/composite primary keys | Preserve ordered parts; use actual `@key` capability, including required/non-null and generation constraints; reject incompatible mappings |
| Declared FKs | Preserve all paired columns/actions. Supported forward mapping must be exact; no name-based inference. Nonstandard match/deferral/SET DEFAULT semantics block automatic conversion |
| Namespaced/unusual names and collisions | Keep physical identity; apply deterministic registry and checked aliases, or reject an unrepresentable alias |

Initial readers target the already declared evaluation families: MariaDB **11.4
with InnoDB**, PostgreSQL **16**, and the bundled SQLite engine recorded by the
driver/test run. MariaDB unsigned/generated/native details, PostgreSQL
schema-qualified types/sequence/identity/special indexes, and SQLite affinity,
rowid/generated/special-index details are explicit adapter responsibilities.
There is no unverified MySQL promise or claim that catalog discovery establishes
application-row decode compatibility. See [support](support-policy.md).

## Shared proof and boundaries

The crate's tests assert canonical ordering, composite part order, cycles,
selection/missing/dangling facts, native-detail serialization, blocking losses,
and redacted operational errors. Its opt-in `test-support` feature provides small
canonical/selection/assertion helpers for independently authored adapter fixtures.
Each adapter must create its fixture with hand-written SQL and prove read-only
credentials, unchanged schema/data, no application-row reads, selection behavior,
metadata visibility, and required loss findings. Readers do not import one another.

The BSL emitter and CLI remain separate issues. Import does not take over an
existing database's migrations, apply generated SQL, continuously synchronize
schemas, add policy, or create an alternative project setup system. See the
[model/key/namespace semantics](../spec/syntax/models.md),
[relations](../spec/syntax/relations.md), [raw boundaries](../spec/syntax/raw.md),
and [principles](../spec/principles.md) when implementing the mappings.
