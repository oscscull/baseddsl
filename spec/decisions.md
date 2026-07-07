# decisions.md

Implementation decisions the prose specs left open. Each resolves an ambiguity the
grammar and compiler must commit to. Governed by `principles.md`; where a default is
chosen it follows principle 2 (omission must have one safe meaning).

Status: proposed. These are my reads of the spec — flag any you'd decide differently.

## Topic index

Decisions are a chronological log (D1→D50); this routes by topic so you can load only the
relevant entries instead of scanning. A decision may appear under more than one topic.

- **Types & schema fundamentals** — D1 (`Id`/PK), D2 (implicit `id` + timestamps), D3 (table/column
  naming), D6 (one extension, uniform grammar), D7 (identifier casing), D8 (contextual keywords),
  D9 (free layout)
- **Manifest & discovery** — D5 (project manifest + `**/*.bsl` glob)
- **`$ctx` (per-request context)** — D4 (inferred, never a global type)
- **Scope / auth** — D19 (`@tenant` removed; `@scope` open), D32 (`@scope` resolved: single-owner
  filter + create auto-set + `unscoped`), D33 (shard key ← scope `$ctx` field), D34 (`@scope` in a
  joined `ON`), D46 (named scope, spec), D47 (multi-scope DNF, spec), D48 (named scope, impl),
  D49 (multi-scope DNF, impl + E0186), D50 (scope editor surface + snapshot serializer)
- **SQL codegen — DDL** — D10 (type mapping)
- **SQL codegen — query reads** — D11 (query SELECTs), D14 (named-filter body resolution)
- **SQL codegen — mutations/writes** — D12 (mutation writes), D16 (tx back-refs `^`)
- **Indexing** — D15 (index inference, baseline emission, lints)
- **Relations** — D17 (custom `on:` join resolution)
- **Query / shape codegen** — D11 (SQL DML mapping), D55 (nested to-one shape sub-objects),
  D57 (to-many nested arrays: correlated-subquery JSON aggregation + self-ref aliasing)
- **Pagination** — D56 (keyset-cursor pagination: lexicographic `WHERE`, hidden cursor-basis columns,
  opaque validated cursor)
- **Client codegen** — D13 (typed Rust client), D30 (typed per-callable `$ctx` in the client)
- **Polyglot / OpenAPI** — D23 (OpenAPI over gRPC, rationale), D24 (OpenAPI emitter shape)
- **Runtime architecture** — D18 (in-process, not artifact-consuming), D20 (serving model: sync +
  bounded pools, single-shard scale-out), D22 (in-process `embed` door), D25 (write-retry
  idempotency), D26 (health/readiness + graceful shutdown), D31 (idempotency key fingerprint)
- **HTTP listener** — D21 (`based serve` + multi-dialect readiness)
- **Dialects & drivers** — D27 (SQLite backend), D28 (SQLite DDL), D29 (Postgres dialect + `$n`
  scanner), D38 (Postgres driver + live suite)
- **Testing / integration harness** — D35 (Docker-backed real-DB harness + MariaDB live suite)
- **Editor / LSP** — D36 (VS Code thin LSP client), D40 (per-file manifest resolution), D43
  (go-to-def + type coloring), D44 (document symbols + capability audit), D45 (completion),
  D51 (field-reference go-to-def + broad hover + clickable inverse inlay), D52 (find-references +
  filter go-to-def), D53 (rename + prepareRename), D54 (workspace symbols ⌘T)
- **Migrations** — D37 (migration generation, spec), D39 (snapshot + diff engine), D41 (per-dialect
  renderer), D42 (apply + `_based_migrations` ledger)

## D1 — `Id` type, default PK = uuid
`Id` is a primitive scalar: the opaque primary-key type. The concrete column type of the
implicit `id` is **`uuid` by default** (distributed-friendly, non-enumerable; MariaDB native
`UUID` where available, else `BINARY(16)`).
- A model whose key is something else declares it explicitly (deviation visible, principle 2).
- `Id` is the type of the implicit `id` column and of any relation's foreign-key value.
- In query params, `org: Id` means "the key of the referenced row." Same-name binding
  to a relation field (e.g. `org`) compares against that relation's FK column.

## D2 — `id` implicit; timestamps are decorated, never implicit
Only **`id: Id`** is implicit (single safe meaning; principle 2 holds). Created/updated
timestamps are NOT implicit — making them so would force `updated_at` onto tables that don't
want it, violating principle 2. They are a real declared field plus a decorator that marks the
engine-managed role, exactly parallel to `@soft_delete` pointing at `deleted_at`:
```
@created(created_at)        # set on insert
@updated(updated_at)        # set on insert + every update
product {
  created_at: timestamp
  updated_at: timestamp
  ...
}
```
A query may only sort by `created_at` if that model declares it (see the product example).
`deleted_at` likewise is a real declared field marked by `@soft_delete` (soft-delete.md).

## D3 — Table & column naming: verbatim, never pluralized
- Table name = `snake_case(ModelName)`, no pluralization. `Order` -> `order`,
  `OrderItem` -> `order_item`. (snake_case only — capitalized SQL table names break across
  case-sensitive/insensitive platforms. No pluralization — irregular plurals are a footgun and
  a magic dictionary violates principle 4.)
- Column name = the field name verbatim.
- Relation field FK column = `<field>_id`. `placed_by: User` -> column `placed_by_id`.
- Legacy override hooks (the base state we're targeting may use any names, including reserved
  words — D8): `@table("legacy_name")` on the model; `(column "legacy_name")` on a field;
  `(on: ...)` on a relation (relations.md). The convention is the default only, never a
  requirement on the existing database.

## D4 — `$ctx` (per-request context; inferred, never a global type)
`$ctx` is a reserved param namespace holding caller-supplied request context (auth.md
Handles 1 & 2). `$ctx.org` is a path into it.
- For the parser: `$ctx` is a normal `$`-param whose name is `ctx`, followed by a dotted path.
- **There is no single "ctx type."** `$ctx` is per-*request* — the caller builds one context bag per
  call. Correspondingly there is no global declaration: each **callable requires exactly the
  `$ctx.<field>`s it reads** (its `where`, its target model's `@scope`, expanded filter bodies,
  `create`/`update` assigns). A public query requires none. This is the honest unit — "the ctx *this
  request* needs" — not "the ctx." (An earlier iteration put a `[ctx]` table in `based.toml`; that
  encoded the "one global ctx" fallacy and was removed.)
- **A field's type is inferred from use, not declared** (implemented, `based-sema::ctx`; sema resume
  #4). `where (org = $ctx.org)` types `$ctx.org` as `org`'s FK — the same inference untyped query
  params already use (queries.md). Uses with no column to infer against (a literal, a raw block, a
  `guard` arg) contribute nothing.
- **Coherence is the one global fact** (closed-world, D5): every callable reads the same caller-built
  bag, so a field *name* must mean one *type* everywhere. A clash — across callables *or* within one —
  is `E0161`. Structural rule: `$ctx.<field>` is exactly one segment (`E0160`); the fields are flat.
- The inferred requirement is attached per callable (`RQuery`/`RMutation.ctx_requires`) and is what
  the generated client will send as request context (one `Ctx` shape *per route*, not a monolith).
  A relation-typed field carries the model's key (D1); codegen renders it `:ctx_<field>` (D11).
- **Residue (deferred):** a `$ctx` field used *only* where inference can't reach — a `guard` (Handle
  3, which takes no args yet) or a raw block — is typed by a **local annotation at the use site** when
  `guard` grows args. No central registry, ever. Also deferred: `$ctx` passed *as a filter arg*
  (arg/usage typing, D14).

## D5 — Project manifest & schema discovery
A project root holds a manifest `based.toml` (name TBD) declaring:
- a format/schema version (room for migration as the language evolves),
- the dialect compile target (default `mariadb`),
- the schema source root (default the project root),
- the generated-client target language (`rust` for now).
The manifest globs `**/*.bsl` under the schema root into the schema = the closed set of
declarations. Closed-world is required by calling.md (index inference, N+1 lint, etc.).
(`$ctx` is **not** declared here — its shape is inferred per callable from use, D4. Closed-world is
exactly what makes that inference + its coherence check decidable.)

## D6 — One extension, uniform grammar
Single file extension `.bsl`. The grammar is uniform: any declaration (model, shape, query,
mutation, named filter) may appear in any file (grammar.ebnf `file = { decl }`). No
per-kind grammar, no "one model per file" rule. Concatenating every `.bsl` file yields the
same schema as any other partition of the same declarations.

(Extension `.bsl` is provisional; it is short and not in meaningful real-world use. Swapping
it is a one-line change in the manifest/discovery glob.)

## D9 — Free layout; recommended convention
Directory structure is the user's choice — **not enforced**. The compiler only globs `*.bsl`.

Recommended convention (what `spec/examples/commerce` demonstrates): one directory per domain
entity, with the schema and its read contracts together and the access layer separate:
```
order/
  model.bsl       # the `order` model + shapes `from Order`
  queries.bsl     # queries / mutations / filters for Order
```
A team may instead split shapes into `shapes.bsl`, put everything in one file, or group by
feature — all parse identically. The convention is guidance, never a constraint.

## D7 — Identifier casing (load-bearing, from models.md). No fold.
Casing distinguishes models from columns at **every** position — declaration and reference
alike — so the reader and lexer classify an identifier by its case alone:
- A **model** is `UpperCamel` everywhere: declared `Order { }`, referenced `Order`. Declaration
  name == reference name (no fold). `OrderItem` is declared and referenced identically.
- A **column / field / primitive** is `lower_snake`: `placed_by`, `created_at`, `text`.
- Rule (models.md): capitalized => model/relation; lowercase => column/primitive.
- The only `UpperCamel -> snake` transform is **table-name generation** (D3), done at codegen,
  overridable via `@table`. Source identifiers never need folding to relate decl and ref.

## D8 — Keywords are contextual (positional), not globally reserved
Keywords are recognized **positionally**: an identifier is read as a keyword only where the
grammar expects one, and may otherwise be used as an ordinary identifier. The set of words
that are keyword-in-position:
`get list create update delete restore hard tx where order page offset count with guard
from shape scope scoped query mutation filter unindexed unsafe unscoped on column table by has in
not and or true false null now`.

Why not global reservation: the canonical `OrderItem` model names a field `order:` (a would-be
reserved word), and the same word is the `order (...)` clause keyword — so the two coexist in
one schema. This is safe because the positional grammar is unambiguous at every decision point:
a model member always starts with `ident :` (field / soft-override) or `@` (index), so `order:`
can only be a field; a query clause is `where`/`order`/`page` followed by `(`, so `order (` can
only be a clause; a statement verb is `get`/`list` at the head of a block. The lexer emits one
`LowerIdent` token for every lowercase word; the parser matches keyword text only in-position.

Tradeoff accepted: a keyword misused where an identifier is expected fails a token or two later
with a generic message rather than "X is reserved." The collision surface is small and the
grammar is tight, so this is worth the friendliness to legacy schemas.

Legacy names are still aliasable (never required): a column literally named with a keyword can be
reached as-is where an identifier is expected, or — if you'd rather not spell the keyword — via
`(column "order")` / `@table("order")` (D3). The alias is greppable and lives in one place
(principle 4); it is now a convenience, not a requirement for adopting a legacy schema.

## D10 — SQL type mapping (MariaDB DDL codegen)
The physical SQL type each primitive lowers to (`based gen sql`, dialect `mariadb`). Chosen for
correctness-by-default over a literal name match — an unbounded/overflow-prone default is a silent
footgun, and this is a DB-first tool:
- `text` -> **`VARCHAR(255)`**. Bounded so the column is directly index/unique-able (MariaDB cannot
  index a `TEXT` column without a prefix length). The convention default; length tuning is a later
  concern (no length primitive exists yet).
- `int` -> **`BIGINT`**. The single `int` primitive must not silently overflow on the common
  money/count use (prices, totals in the commerce example).
- `bool` -> **`BOOLEAN`** (MariaDB alias for `TINYINT(1)`).
- `timestamp` -> **`DATETIME`** (not `TIMESTAMP`): dodges MariaDB's implicit `ON UPDATE
  CURRENT_TIMESTAMP` on the first timestamp column and the 2038 range cap. `@created`/`@updated` are
  engine-managed explicitly, so no implicit column behavior is wanted.
- `date` -> `DATE`; `json` -> `JSON`.
- `uuid` / `Id` -> **`UUID`** (native MariaDB type, D1). The implicit `id` is app-generated, so it
  gets no SQL `DEFAULT`; a relation's FK column takes the target's PK type (default `UUID`).
- A to-many scalar (`text[]`) has no columnar form -> `JSON` (a JSON array).

Relations emit the FK *column* (`<field>_id`) but **no** `FOREIGN KEY` constraint — constraints are
opt-in (relations.md). `(unique)` -> a `UNIQUE` constraint; `@index` -> an inline `KEY` / `UNIQUE
KEY` whose column list resolves relation fields to their FK columns. All identifiers are backtick-
quoted (legacy/reserved names like `order`, `user` are common, D8).

## D11 — SQL DML mapping (MariaDB, query SELECTs)
How a `query` lowers to SQL (`based gen sql` query section, `based-codegen::sql::dml`). The read
side; mutations are a later increment.
- **Root alias = the table name** (`FROM \`order\``); every join is aliased `j_<path_prefix>` with
  dots -> `_` (`address.city` -> `j_address_city`), so joins dedupe by the path prefix they traverse
  and a shape + a `where` reaching the same relation share one join.
- **Relation reaches become joins; a terminal relation is its FK column.** `placed_by.name` joins
  `user` and reads `name`; a single-segment relation in a filter (`org = $org`) compares the local
  FK (`order.org_id`), no join. Forward-optional -> `LEFT JOIN`, forward-required -> `JOIN`
  (inner), inverse/to-many -> `LEFT JOIN`.
- **Soft-delete + `@scope` injection is the headline guarantee** (soft-delete.md, auth.md). The
  tombstone predicate is added to the root `WHERE` and to *every joined table's `ON`* (keeping a
  `LEFT JOIN` a left join). Covered subset -> predicate: timestamp/date `IS NULL`, bool `= FALSE`.
  `@scope(pred)` is lowered like any `where` and ANDed on. The user writes neither.
- **Parameters render as named `:name` placeholders** (`$ctx.org` -> `:ctx_org`), not MariaDB's
  native positional `?`. Named keeps the emitted template legible (readable > terse, CLAUDE.md);
  the generated client (M4) translates to the driver's binding form.
- **Sort cascade**: query `order (...)` > model `@sort` > none (sema lints the empty `list`). Keyset
  pagination (a `page` without `offset`) appends `\`root\`.\`id\` ASC` as the unique tiebreaker —
  shown in the SQL, never written in source (principle 8, pagination.md).
- **Pagination**: `page (N)` -> `LIMIT N`; `offset` -> `... OFFSET :offset`; `with count` -> a
  second `SELECT COUNT(*)` over the same FROM/JOIN/WHERE (live rows, no LIMIT). The keyset cursor
  comparison itself is a runtime concern and is *not* in the generated base SELECT.
- **Bare bool** in a predicate (`where (... and active)`) -> `\`t\`.\`active\` = TRUE`. Operators:
  `~` -> `LIKE`, `in` -> `IN`, `has` -> `MEMBER OF` (JSON array containment), the rest verbatim.
- *Deferred, rendered visibly rather than wrong*: nested shape sub-objects are skipped (need JSON
  aggregation); a named-filter call in `where` becomes `TRUE /* filter … deferred */` (filter bodies
  aren't resolved against the call site yet, sema resume #2).

## D12 — SQL DML mapping (MariaDB, mutation writes)
How a `mutation` body lowers to SQL (`based gen sql` mutation section, `based-codegen::sql::mutations`).
The write side; reuses D11's join resolver so a mutation `where` lowers identically to a query `where`.
- **`delete` on a `@soft_delete` model is rewritten to the tombstone UPDATE — never a real DELETE**
  (the headline write-side guarantee, soft-delete.md). `restore` is the inverse (clears the tombstone).
  `hard delete` is the loud, explicit opt-out that *does* emit a real `DELETE`. A plain model's `delete`
  is a plain `DELETE`. Covered subset: timestamp/date -> `CURRENT_TIMESTAMP`/`NULL`, bool -> `TRUE`/`FALSE`.
- **Injected guards.** The soft-delete live predicate and `@scope` (auth.md) are ANDed into every
  UPDATE/DELETE `WHERE`, so a write can't touch a tombstoned or out-of-scope row — same injection the
  read side does. Exceptions: `restore` skips the *live* predicate (it targets deleted rows) but keeps
  `@scope`; `hard delete` skips the *tombstone* (that's the point) but keeps `@scope`.
- **Relation-reaching `where`** lowers to MariaDB's multi-table forms: `UPDATE m JOIN j ON … SET … WHERE …`
  and `DELETE m FROM m JOIN j ON … WHERE …` (single-table otherwise: `UPDATE m SET …` / `DELETE FROM m`).
- **Engine-managed columns.** `create` binds the app-generated `id` as `:id` (D1 — uuid, no SQL default),
  skipped only if the model declares its own `id` the caller sets. `@created`/`@updated` are set to
  `CURRENT_TIMESTAMP` on insert (D2 — no DB default); `@updated` is bumped on every UPDATE, including the
  soft delete/restore rewrites. All engine columns are skipped when the caller assigns them explicitly.
- **`tx { … }`** renders its inner writes in declaration order under one engine-owned transaction
  (principle 7): the engine, not the emitted SQL, owns BEGIN/COMMIT — no transaction control is emitted.
- **Parameters** render as named `:name` placeholders, same as D11 (`$ctx.org` -> `:ctx_org`).
- **Declared-shape return via re-select (done).** A mutation that **creates** its return row emits a
  trailing re-select — `SELECT <return shape> FROM <return model> WHERE id = :result_id [AND <live> AND
  <scope>]` — reusing the read side's `project_return`, so the projection can't drift from a `get` of the
  same shape (principle 4). MariaDB *has* `INSERT … RETURNING`, but re-select was chosen: it is
  dialect-portable (Postgres/SQLite/MySQL alike), handles the relation-reaching shape joins uniformly, and
  reuses the one read-side projector rather than a second RETURNING path. The runtime (`plan_mutation`)
  binds `:result_id` to that create's engine id and runs the re-select **inside** the write transaction
  (read-your-writes, atomic with the writes). A pure update/delete has no engine id to key on, so it emits
  no re-select and the response falls back to `{ id }`/`{}` (its re-select would key off the write `where`
  — cardinality-ambiguous, still deferred). The live + `@scope` guards ride the re-select exactly as a
  `get` would, so a create that lands out of scope reads back as absent.
- *Deferred, documented not silently wrong*: `^` tx back-references (`user = ^.id`) — not in the
  lexer/AST (sema resume #6), so a `tx` is a flat independent statement sequence; required-field
  enforcement on `create` (sema resume #7) — an INSERT omits unassigned non-optional columns rather than
  erroring; a raw write statement has no attached model, so `{table}`/`{id}` interpolation has no root.

## D13 — Typed client mapping (Rust, `based gen client`)
How a `query`/`mutation` signature lowers to a typed client (`based-codegen::client`). The manifest
`client` target selects the emitter; Rust is first and the default. calling.md's closed RPC surface:
each signature generates exactly one input type, one output type, and one wire route — the wire carries
arguments, never the DSL.
- **One route per callable**: `POST /q/<name>` for a query, `POST /m/<name>` for a mutation. The route
  is a `const` (`<NAME>_ROUTE`); a `Client<T>` method posts the input struct and decodes the output.
- **Input struct** = one field per signature param. An explicit annotation wins (a *model* type ->
  `Uuid`, the FK the wire carries, D1); an untyped param is inferred from the column it maps to (an
  `-> edge` / same-name relation param -> `Uuid`; an `op col` / same-name scalar -> that column's type).
  A param with a `(default)` or an optional annotation -> `Option<T>` (client may omit; engine applies
  the default). `$ctx` is server context (auth.md), never a client input.
- **Output type** from `-> Output`: a declared shape -> a struct projecting its body (each field typed
  by the column it reaches; a relation reach's terminal FK is `Uuid`); a bare model / `full` -> a struct
  of every stored column (forward FKs as `Uuid` under the relation field name, matching the SELECT alias).
  A shape shared by two callables emits one struct.
- **Return wrapper** (query): paginated (`page` without `offset`) -> `Page<T>` = `{ rows, cursor }` (the
  envelope, calling.md — never a bare array); other `list`/many -> `Vec<T>`; a `get` (single) ->
  `Option<T>` (a keyed lookup may miss). A mutation returns the single `T` (or `Vec<T>` if `-> T[]`).
- **Type aliases** mirror the DDL side (D10): `Uuid`/`Timestamp`/`Date` alias `String`, `Json` aliases
  `serde_json::Value`; `optional` -> `Option<T>`, to-many scalar -> `Vec<T>`. A `sql`…`` shape field
  has no static type -> `Json`. Field names colliding with a Rust keyword are `r#`-escaped (`type` ->
  `r#type`).
- **Transport is abstract.** The runtime (not started) owns HTTP/driver binding, so codegen emits
  `Client<T: Transport>` where `Transport::call(route, &input) -> Result<O, ClientError>` — the typed
  surface without an invented HTTP stack. The generated module needs only `serde` + `serde_json`.
- *Deferred, same as the read side*: nested shape sub-objects (`field { … }`) are skipped in the output
  struct (need JSON aggregation, PLAN M3); the keyset cursor is an opaque `Option<String>` (runtime).

## D14 — Named-filter body resolution (call-site, `$`-params)
A `filter name(params) = predicate` declares no model, so its column paths belong to no table until it is
*used*. queries.md left two things open; both resolved here.
- **Params are `$`-referenced.** A filter param is written `$c` in the body, identical to `$param`
  everywhere else — the grammar's `param_ref = '$' param_name` already requires it, and "$ means bound
  parameter everywhere" (queries.md) is the one rule. A bare `c` in a body is a *column*, not the param.
  (queries.md's old `= c` example was pre-grammar prose; corrected to `= $c`.)
- **Body columns resolve at each call site**, not at declaration. When a `where` / `@scope` / another
  filter references `f(args)` (or a zero-arg `f` used as a bare atom), sema re-resolves `f`'s body against
  the *caller's* model, with the filter's own params as the legal `$`-set. Column-name errors (`E0111`),
  traversal errors, and operand typing (`E0150`/`E0151`) all fire against the real model — so the same
  filter reused on two models is checked twice, once per shape. At declaration only params / nested-filter
  arity / functions are checked (no model to bind columns to).
- **Cycles terminate.** A filter that expands to itself (directly or transitively) is guarded by an
  in-progress stack and simply stops re-expanding; recursion is not *rejected* (no error), just bounded.
- *Deferred*: propagating soft-delete injection *through* a filter call into codegen (M3 read still
  renders filter calls as a `TRUE /* deferred */` no-op — sema now resolves them, but the SQL lowering of
  a filter body is a separate codegen pass); type-checking each `arg` against how its param is used in the
  body (filter params carry no declared column, so arg/usage agreement is unchecked).

## D15 — Index inference, baseline emission, and the index lints
How indexing.md's inference + bidirectional lint lower (`based-sema::indexes`; DDL emission in
`based-codegen::sql`). Closed-world (D5) is what makes this decidable: the access layer *is* the
full set of generated SQL, so "a query will scan" and "no query uses it" are facts, not guesses.
- **Inferred baseline = traversed join keys only.** Every inverse edge some query traverses — via a
  `where`/`order`/`@scope` path, a shape reach, or a nested sub-object — needs an index on the FK
  column the join runs through (the forward `via` field's `<field>_id`). That is the one class of
  index that is unambiguously right to auto-create, so it is emitted in DDL as
  `KEY inf_<table>_<cols>` (the `inf_` prefix marks engine-owned keys), deduped against declared
  structure. Forward joins land on the target's PK, already indexed. On a `@soft_delete` model the
  tombstone column is physically prepended (`(deleted_at, order_id)`): MariaDB has no partial
  indexes, so "predicate-leading" (indexing.md) means leading with the always-filtered column.
- **Filter-path indexes are shown, never auto-created** (principle 8). Whether one is worth its
  write tax is a human call, so they surface as the W0103 lint and the human answers with an
  `@index` or an `unindexed` annotation.
- **W0103 `unindexed` (missing-index).** Per query, sema collects the root-table access pattern:
  equality fields (`=`/`in`/bare bool — including param bindings on the bare/inline tiers, the
  model's `@scope`, and named filters expanded at the call site, D14), range fields
  (`< > <= >= ~`), and the leading local sort field (query `order`, else model `@sort`). The query
  is *served* if any available index — PK, unique columns, declared `@index`es, inferred join
  keys — leads with one of the eq/range fields; a declared index's own leading soft-delete column
  is skipped when finding its lead. With no filter at all, only a *paginated* list is checked,
  against its sort key (an index there is early-exit instead of full filesort). Unserved → W0103.
  A pattern containing `or` or a raw atom is opaque: the lint stays silent rather than guess
  (precision over recall — a warn lint must not cry wolf). Every table is treated as
  plausibly-consequential today ("unknown", indexing.md — there are no prod stats yet);
  `max_rows` is carried for the future stats ratchet.
- **The annotation is a query clause** (grammar `unindexed_clause`), legal wherever
  `where`/`order`/`page` are: `unindexed(max_rows: 500)` / `unindexed(unsafe[, "reason"])`.
  W0105 flags a stale annotation (the query turns out indexed — drop it). Surfacing `unsafe` in
  an audit listing is deferred (a CLI/LSP concern, not a check-time diagnostic).
- **W0104 `useless-index`.** A declared non-unique index whose effective lead no query filters,
  sorts, or joins on is pure write tax. Usage is pooled broadly — both `or` branches count, and a
  column reached *through* a relation counts against the model it lives on — so the lint
  under-fires rather than over-fires. Exempt: unique indexes (constraints, not perf) and an index
  leading with the soft-delete column (used by construction). A single-column index duplicating a
  `(unique)` constraint is flagged regardless of queries.
- *Deferred, recorded not silent*: mutation `where` patterns don't feed W0103 (write-side scans);
  index matching is lead-column only (no composite-prefix/permutation reasoning); prod-stats
  floors + `max_rows` re-checking; the LSP surface showing inferred indexes inline (M5).

## D17 — relation `on:` custom-join resolution (relations.md)
A forward relation may override the convention FK with a legacy-key join:
`placed_by: User (on: order.user_ref = user.legacy_id)`. Unlike every other predicate this one
spans *two* tables and refers to columns table-qualified, so it needs its own resolution scope.
- **Two-table scope = the FK-holding model + the relation target.** The only two tables a to-one
  join can touch. Each column path must be exactly `<table>.<column>` where `<table>` matches one of
  the two models' physical table names and `<column>` is a physical column on it. Resolved in
  `model::resolve_exprs` (the read pass — the target model must already be built) via
  `resolve::check_relation_on`.
- **Match physical columns, not field names.** Joins are written in DB terms (legacy keys), so
  resolution goes through `RModel::column` (a scalar's `column` override or a forward's `fk_col`),
  not `member`. A field with `(column "legacy_id")` therefore resolves by its column name.
- **Codes.** Unknown table qualifier → `E0125`; unknown column → `E0111` (reuses the field code with
  column wording); malformed join → `E0126` — a path that isn't `<table>.<column>`, a `$`-param /
  function / `^` back-ref / named-filter (a join is static structure, no request scope), or `on:` on a
  field that isn't a to-one relation (a `[]`/inverse edge owns no FK; a scalar owns no join).
- *Deferred*: self-ref joins resolve against the one model on both sides (aliasing the two logical
  sides is a codegen concern); **lowering** the custom predicate into the emitted JOIN — codegen still
  joins on the convention `fk_col`, so `on:` is checked but not yet honored in SQL.

## D16 — tx back-references (`^`, mutations.md)
`tx { create A{…}; create B{ x = ^.id } }` wires a just-created row's key into the next write. How it
resolves and lowers (lexer `^` token → AST `Value::Back(BackRef)` → parser → `based-sema` → `based-codegen`).
- **`^` reads the *immediately preceding* `create`** in the enclosing `tx`. Not "any prior step" — the
  most recent create only. Simple, matches the spec example (`create User; create Address{ user = ^.id }`),
  and needs no step labels. `^.field` reads a column of that created row; `^.id` (the FK-wiring case) is
  the overwhelming use.
- **Scope is the `tx`.** A `^` at the first statement of a tx (nothing precedes), in a plain non-tx
  `create`, or in a query/predicate position is a misuse → `E0170`. An unknown `^.field` → `E0111`
  (resolved against the preceding create's model, *not* the model being written).
- **Codegen: sibling creates get distinct id binds.** Ids are app-generated (D1), so within a `tx` each
  `create` at step *k* emits its id as `:id_<k>` (top-level lone creates keep `:id`). This both fixes the
  latent collision (two creates both binding `:id`) and gives a back-reference a name: `^.id` lowers to the
  prior create's `:id_<k>`. `^.<other>` reuses the value that create assigned to the field (a caller
  `:param`/literal), which the engine already binds.
- *Deferred*: `^.field` for a column the prior create did **not** set (engine default, or read-after-write)
  emits a visible `NULL /* ^.field … */` marker — recovering it needs a re-select/RETURNING, a runtime
  concern; multi-level `^^`; back-ref *type* agreement with the assigned column (like D14 filter args,
  the referenced value carries no re-checked column type at the use site).

## D18 — runtime architecture (M6): in-process, not artifact-consuming
The runtime (`based-runtime`) is **in-process**: it links `based-sema` + `based-codegen`,
holds the same `CheckedSchema` the compiler produced, and reuses codegen's *one* lowering
(`sql::lower_queries`) to get each callable's SQL. It does **not** read the generated `.sql`
text back, nor a separate serialized bind-manifest.
- **Why.** Principle 4 (one source of truth): the executed SQL and its bind surface come from
  the same lowering that `based gen sql` emits, so they cannot drift. The artifact-consuming
  alternative would declare bind order / envelope / input types a *second* time (a manifest to
  keep in sync) and re-scan generated SQL to recover `:name` order — the exact fragility P4
  exists to prevent. Its only win — a deployment boundary where the runtime never sees `.bsl`
  — is not a goal (audience = one workspace, LLMs + reviewers).
- **Cost paid.** Codegen now exposes structured per-query lowering (`LoweredQuery { sql,
  count_sql }`); the text emitter (`render_query`) renders from it, so the `based gen sql`
  goldens are unchanged. Mutations will get the same treatment when the write path lands.
- **Named→positional is a runtime/driver concern.** Codegen keeps legible `:name` placeholders
  (D11); the runtime translates to MariaDB's `?` with a quote-aware scanner, resolving each
  name from one value environment built from the validated args + `$ctx` + pagination. The
  placeholder *names* are unambiguous given the schema (`:<param>` / `:ctx_<field>` / `:offset`),
  so nothing parallel to the SQL is maintained.
- **`Db` is the seam** (twin of the client's abstract `Transport`, D13): the read path needs
  only `fetch`; a `MockDb` makes the whole request→JSON path testable with no database. The
  concrete MariaDB driver reuses a hardened external one (principle 7), next slice.
- *Deferred*: the write path (mutations — engine id-gen behind a deterministic `IdGen` seam,
  `tx` under one engine-owned transaction, write-response); the concrete driver; the HTTP
  server (`based serve`, the `POST /q|m/<name>` wire surface, calling.md).

## D19 — `@tenant` removed; `@scope` injection semantics are OPEN
**Decided: `@tenant` is gone.** It was a validated-but-inert decorator (recorded the field,
injected nothing) whose relationship to `@scope` was undefined — the worst quadrant of
principle 1 (reads like an isolation guarantee, silently enforces nothing) and a principle-4
duplication of `@scope`, whose own canonical example (auth.md Handle 2) *is* the tenant case
`@scope(org = $ctx.org)`. Tenant isolation is not a distinct language feature: it is the
single-owner instance of a scope predicate. Removed from grammar-known decorators, `RModel`,
the resolver, and the conformance summary; commerce/product now carries no tenant decorator.
Multi-owner ("writable by a *set* of orgs") is not even single-tenant scoping — it is a
caller-computed filter value (auth.md Handle 1, `org in $ctx.writable_orgs`), further evidence
`@tenant` modelled the wrong shape.

**RESOLVED by D32** (the four axes below are settled there; read D32 for the shipped design).
The injection itself was in fact built earlier (read `WHERE` + write `WHERE` + `$ctx`
propagation); D32 closes the gaps that made it safe — the create-time hole, the escape hatch,
and the predicate restriction. The original open axes, for the record:
1. **Per-model vs per-operation.** Model-level `@scope` injects the *same* predicate into every
   query on the model (soft-delete-like, auth.md:18). Real access rules differ by operation
   (broad read, owner-only write). Decide: is scope attachable per query/mutation, or is
   non-uniform scoping always hand-written as a per-query `where` (Handle 1)? If the latter,
   `@scope` is only ever correct when the predicate is *genuinely uniform across all ops* — say
   so, or it will be misapplied.
2. **Boundary vs `@scope`/guard.** The role/permission matrix stays out (principle 5, auth.md:6
   — app logic, host seam, Handle 3 guards). `@scope` must own *only* the uniform, compiler-
   guaranteeable row filter — and inherit soft-delete's cross-join correctness. Nail the line so
   it doesn't creep toward being a policy engine.
3. **Escape hatch (principle 6).** Cross-scope access (admin/support/jobs/provisioning) is
   inevitable. Injection needs a *mandatory, minimal-scope, greppable, linted* opt-out before
   anyone relies on it — else the first admin query disables the whole mechanism. No design =
   not shippable.
4. **What the compiler guarantees vs cannot.** It can guarantee the predicate is injected
   everywhere (kills accidental cross-scope leaks); it cannot verify the role matrix is correct
   (that lives in host guards). Document this honestly so `@scope` is not mistaken for a checked
   authorization model.

## D20 — runtime serving model: sync + bounded pools, single-shard scale-out
Target is **scaled-enterprise, very high load, long-term uptime, low complexity**. The
serving architecture that meets that bar:

- **Sync, not async.** The `Db` trait stays synchronous; the server is a bounded worker-thread
  pool over a bounded connection pool. Async was evaluated and rejected *for this workload*:
  - The real concurrency ceiling is the **DB connection pool**, which you must bound in either
    model to protect a MariaDB box (per-connection server memory + lock contention past a few
    hundred–low-thousand active conns). Async does not raise that ceiling.
  - Async's genuine win is cheap *idle* sockets (C10k). This is a short-request RPC service
    behind a load balancer, which terminates keep-alive/slow clients — the app sees only active
    requests, so that win doesn't apply.
  - For DB-bound work each request blocks ~0.2–5 ms on a MySQL round-trip; a thread
    context-switch is ~1–5 µs, so async's scheduling efficiency saves a cost that isn't ours.
    200 blocked worker threads × 512 KB stack ≈ 100 MB — negligible.
  - Async's cost is real and permanent (infects `run`/`serve`, `Send+'static`, cancellation
    safety, executor-starvation footguns) — directly against "very dependable, low complexity."
  - The one case where async would pay — in-process cross-shard **scatter-gather** — is
    explicitly out of scope (single-shard, below). If direct streaming to slow clients without a
    buffering proxy ever becomes a requirement, revisit; it's a bounded `run`/`serve` rewrite.
- **Scale for load horizontally**: shards + more app instances behind an LB, not
  threads-per-process. Bounded pools cap per-box concurrency; capacity is added by adding boxes.
- **Single-shard per request, no scatter-gather.** A request routes to exactly one physical
  shard. Consequences that buy dependability + simplicity: a mutation's `tx` is one shard →
  engine-owned BEGIN/COMMIT with **no distributed transaction/2PC**; a down shard fails only its
  own traffic; the router stays trivial (pick one pool). Cross-shard/analytics is a separate
  read-replica/warehouse path, off the hot path — not expressible on the RPC wire.
- **Route through a fixed logical-shard space, not `hash % N`.** A key is FNV-hashed (a *stable*
  algorithm — `DefaultHasher` is not stable across releases and would strand data) into a
  permanent `LOGICAL_SHARDS` space (4096), then a small `logical→physical` assignment maps it to
  a pool. Adding a physical shard moves *whole logical shards* (a bounded data migration) without
  rehashing any key (the Vitess/Citus model). This is what preserves usability as the fleet grows.
- **`Db` is fallible; writes are all-or-nothing.** Every `Db` method returns `Result<_, DbError>`
  (a dependable driver surfaces connection/timeout/deadlock failures, never panics); a mutation
  rolls back on any write error. The wire maps a `DbError` to a **retryable 503** (the generated
  SQL comes from a checked schema, so runtime DB errors are operational, not query bugs), distinct
  from a boundary `PlanError` → 4xx.
- **Driver = reuse, not hand-roll (principle 7).** The `mysql` crate (pure Rust) + its built-in
  bounded pool; TLS/compression off by default to avoid a system OpenSSL dependency (a deployment
  that needs in-transit TLS re-enables it).
- ~~**Shard-key source deferred, decoupled from D19.**~~ ✅ **resolved (D33).** The natural shard
  key is the tenant/owner — the same field `@scope` uses — which was left pluggable while `@scope`
  was OPEN (D19). D32 resolved `@scope` to a single `col = $ctx.field` equality, so D33 binds the
  shard key to that owner field (schema-derived per callable; `unscoped` → no owning shard; explicit
  `X-Based-Shard-Key` override retained).
- **Known gap — write idempotency.** App-side `id`-gen (D1) means a client retry of a `create`
  after a 503/timeout can double-insert. A dedupe/idempotency key (likely carried in `$ctx`) is
  wanted before write retries are safe at enterprise scale. Not yet designed.

## D21 — HTTP listener (`based serve`) + multi-dialect readiness
The listener (`based-runtime::http`, feature `serve`) is the thin socket edge over the pure
`serve::dispatch` core (D18). Per D20 it is a **sync bounded worker-thread pool** over the
bounded connection pool: N workers share one blocking `tiny_http::Server` (a hardened lib,
principle 7), each looping `recv → decode → dispatch → respond`. Decisions:
- **`$ctx` from headers, never the body** (auth.md, D7). A pluggable `ContextSource` derives
  `$ctx` + the shard key from request headers; the default `TrustedHeaderContext` reads a
  pre-authenticated `X-Based-Context` (JSON) set by an upstream auth proxy (which strips any
  client copy). This is the trusted-edge seam, not an authenticator — policy stays outside the
  runtime (principle 5).
- **Pre-checkout guard.** `serve::preflight` rejects a non-POST / unroutable request *before* a
  pooled connection is borrowed, and `dispatch` runs the same guard, so a malformed-request flood
  can't drain the pool and the two paths can't diverge.
- **Production id-gen.** `UuidGen` (v4 uuids, D1) is built fresh per request — id state is
  per-request, never shared across worker threads.

**Multi-dialect readiness (MySQL / Postgres / SQLite are wanted; not built yet).** The
architecture is deliberately shaped so a second engine is *additive*, not a rewrite. Two seams
carry it:
- **Codegen: the `Dialect` enum** (`based-codegen`). `ddl` / `dml` / `mutations` already branch
  on it (only `MariaDb` today). A new engine = a new variant + its branches (placeholder style,
  quoting, type map, `RETURNING`/upsert availability, tx syntax).
- **Runtime: the `Db` + `Backend` traits** (`based-runtime::run`). `Db` speaks *positional SQL +
  driver-neutral `SqlValue`*, not a MariaDB protocol; `Backend` is the connection source keyed by
  shard. `MariaDb`/`ShardRouter` are one impl; a `postgres`/`rusqlite` impl is a drop-in (SQLite
  ignores the shard key and returns its one connection). **The HTTP edge is fully `Backend`-generic
  — it never names a concrete driver**, so `based serve` needs no change when a driver is added.
- **The one concrete coupling to fix when a non-`?` engine lands:** the named→positional scanner
  (`scan.rs`) hardcodes `:name` → `?` (MySQL/SQLite/MariaDB style). Postgres uses `$1, $2, …`, so
  the rewrite must become dialect-aware (emit `?` or `$n`) at that point. Left hardcoded now rather
  than abstracted speculatively — it is a localized, well-understood change.
- **Coherence rule:** one `Dialect` drives *both* codegen lowering and driver selection — you
  serve Postgres-lowered SQL only to a Postgres `Backend`. The pairing is a deployment invariant,
  not something the wire negotiates.

## D22 — the in-process door (`based-runtime::embed`, Tier 1)
The engine has two front doors over *one* core: run it as a container (`based serve`, D21) or
embed it as a Rust library. `embed::Engine` is the library door — a `Compiled` schema over one
`Db` + one `IdGen`, running a callable through the same `serve::dispatch` core as the HTTP edge,
with **no socket**. An embedded call and an HTTP call take the identical plan → run → shape path
and return the identical `WireResponse` (principle 4 — the doors can't drift). Decisions:
- **Same typed client, no socket.** The generated client (`based gen client`) is generic over a
  `Transport` trait it *defines itself*. By the orphan rule the `impl Transport` bridging it to an
  `Engine` therefore lives in the *embedding* crate, not `based-runtime` — a ~10-line function
  (serialize input → JSON args, `engine.call(route, args, ctx)`, decode the `200` body into `O`;
  non-`200` → the client's `ClientError`). Documented in `embed`'s module docs and proven
  end-to-end in `tests/embed.rs` (the verbatim generated client over a `MockDb`).
- **`&self` via interior mutability.** `Transport::call` is `&self`, but `dispatch` needs `&mut`
  on the connection + id-gen, so `Engine` holds them behind a `RefCell`. This makes an `Engine`
  single-threaded by design (one embedded connection, one thread at a time). A pooled / multi-
  threaded embed routes through the `Backend` seam instead — check out a connection per request
  and build a short-lived `Engine` around it (the same seam `based serve` uses).
- **`$ctx` supplied straight in.** In-process there is no header dance: the app passes the derived
  `$ctx` to `Engine::call` directly (auth.md/D7 still holds — the *app*, not the caller, sets it),
  which is cleaner than the `X-Based-Context` header the HTTP edge needs (D21).
- **Why in-process at all (D20 cost model).** The per-call cost is the DB round-trip (0.2–5 ms);
  over the wire it is *also* the loopback TCP + HTTP framing. Dropping the socket removes that
  framing while keeping the typed client — one binary (no sidecar), steadier latency, `MockDb`
  end-to-end tests, and the path toward **app-owned transactions** (several callables over one
  connection — inexpressible on stateless HTTP RPC). Binding the input struct straight to
  `SqlValue` (skipping JSON) is explicitly *not* pursued: its payoff is nanoseconds against a
  millisecond DB call (PLAN Tier 3).
- **Resolved (D12).** A create-returning mutation's wire response is now the created row read back
  in its **declared shape** (the trailing re-select, D12), so the typed client's mutation method
  decodes clean into that type — the same typed round-trip a `get` gets. `tests/embed.rs` proves the
  verbatim generated `place_order` returns a typed `OrderCard`. (A pure update/delete still responds
  `{ id }`/`{}` — its re-select is deferred, D12.)

## D23 — polyglot clients via OpenAPI, not per-language emitters (and not gRPC)
The container door (D21/D22) exists to serve non-Rust callers, so the schema must yield clients in
many languages. The decision is **how**, and it is deliberately *not* "hand-write a TypeScript
emitter, then a Python one, then Go":
- **A single `based gen openapi` emitter** off the same `CheckedSchema` the Rust client uses. It
  emits an OpenAPI document (`paths` for each `POST /q|m/<name>`, `components.schemas` for the
  input/output types) — one machine-readable contract that `openapi-generator` (or similar) turns
  into a typed client in any language. So polyglot is *one* emitter, not N. This subsumes the
  earlier "second emitter = TypeScript" note (M4/PLAN): TS/Python/Go all fall out of the spec.
- **The Rust client stays hand-emitted** (`based gen client`). It is the in-process `Transport`
  path (D22) — tighter and more useful than a generated HTTP stub would be. `ClientTarget` still
  branches, but only for emitters we hand-write, not for every wire language.
- **Type mapping reuses D10/D13** near-verbatim: `Uuid`/`Timestamp`/`Date` → `string`, `Json` →
  `object`, `int` → `integer`; the `get`/`list`/paginated envelopes → the response schemas; the
  `{ error: { code, message } }` envelope → the error responses. The emitter *documents* the
  surface `serve::dispatch` already serves — it invents no new wire.
- **`$ctx` is not a body param.** It rides the `X-Based-Context` header (D21, auth.md/D7), so the
  spec models it as a header / security scheme, never an input field.
- **Soft prerequisite: D12 (met for creates).** An accurate *mutation* spec needs the declared-shape
  re-select, now landed for create-returning mutations (D12) — so the OpenAPI response schema for a
  create can advertise the declared shape. A pure update/delete still responds `{ id }`/`{}` (its
  re-select is deferred), so the emitter must model *those* as the `{ id }` schema until they follow.
  Read specs are accurate today.

**Why not gRPC** (the obvious "typed polyglot RPC" alternative — its one real draw is that `protoc`
generates clients in ~10 languages, i.e. it also solves the N-emitters problem):
- **Its headline win is void here.** Binary protobuf + HTTP/2 framing saves microseconds against a
  0.2–5 ms DB round-trip (D20) — the same cost logic that killed the JSON-free Tier 3 path. No
  measurable gain.
- **It re-imports the complexity D20 rejected.** The mainstream Rust stack (tonic) is async/tokio;
  D20 weighed and rejected async for the serving model (bounded DB pool is the ceiling in both;
  async's cancellation complexity conflicts with "dependable, low complexity").
- **It penalizes the primary caller.** Browsers/web-TS (the biggest client audience) can't speak
  native gRPC — they need grpc-web + an Envoy-style proxy. Plain `POST`+JSON works from a browser,
  curl, and any LB/gateway/WAF with no proxy (principle 7 — reuse boring, hardened infra).
- **It adds a second IDL.** `.bsl` is already the contract; protobuf would be a parallel schema to
  generate + keep coherent. OpenAPI describes the *existing* JSON wire instead of replacing it.
- **Half of it is unused.** gRPC's real differentiator is streaming; the query/mutation model is
  unary request/response CRUD (queries.md/mutations.md), so the streaming surface goes to waste.
- gRPC would only start to earn its cost as high-fanout internal service-to-service traffic wanting
  deadlines/mTLS/streaming by default — not the stated web/BFF audience. Recorded so it isn't
  re-litigated.

## D24 — OpenAPI emitter shape (`based gen openapi`, delivers D23)
The concrete form of the OpenAPI 3.1 document (`based-codegen::openapi`; CLI `based gen openapi
[--out]`). D23 decided *to emit OpenAPI*; this records *what* the emitter produces. It reuses the
client emitter's collection + type-resolution structure near-verbatim (same `Callable` walk over
the AST, same reach-to-column resolver) so the spec and the Rust client can never disagree on
routes or field shapes (principle 4) — only the leaf mapping differs (Rust types → JSON Schema).
- **One `POST` operation per callable**, keyed by the identical route the wire serves and the
  client emits: `/q/<name>` (query) / `/m/<name>` (mutation), `operationId = <name>`. The request
  body references a generated `<Name>Input` schema (one property per signature param; a param is
  `required` unless it has a `(default)` or an optional annotation — the engine fills the rest).
- **Response schemas mirror the return wrapper** (calling.md, same as D13): a `get` → `oneOf[T,
  null]` (a keyed lookup may miss), a `list`/many → an `array`, a paginated `list` → an inlined
  `{ rows, cursor }` envelope (the `Page<T>` twin — inlined because JSON Schema has no generics),
  a create-returning mutation → its declared shape `T` (D12), an `-> T[]` mutation → an array.
- **Two fixed shared schemas.** `Error` = the `{ error: { code, message } }` envelope
  `serve::dispatch` returns, referenced by the `400`/`404`/`503` responses every operation
  carries (the same statuses D20's dispatch maps). `MutationResult` = the `{ id }` a **pure**
  update/delete responds with — its declared-shape re-select is still deferred (D12), so its
  `200` points here until it follows; a create/model-returning mutation advertises the real shape.
- **Type mapping = D10/D13 re-projected to JSON Schema.** `text` → `string`; `int` → `{integer,
  int64}`; `bool` → `boolean`; `uuid`/`Id`/a relation FK → `{string, format: uuid}` (the wire
  carries the id, D1); `timestamp` → `{string, date-time}`; `date` → `{string, date}`; `json` /
  a `sql`…`` field → JSON Schema `true` (any value). A to-many scalar → an `array`. `optional`
  drops the property from `required` (never a nullable type — absence is the signal, matching the
  client's `Option<T>`).
- **`$ctx` is a header, never a body field** (D21, auth.md/D7). It rides a single reusable
  `components.parameters.BasedContext` header (`X-Based-Context`, a JSON object an upstream auth
  proxy sets) that every operation references. The per-callable `$ctx.<field>` requirements
  (D4/D5, read off `RQuery`/`RMutation.ctx_requires`) are surfaced *descriptively* as an
  `x-ctx-requires` vendor extension on each operation (`{ field, type }`, `type` = the primitive
  name or `-> Model`) — documentation for the caller, not a wire-enforced input.
- **OpenAPI 3.1** (JSON Schema 2020-12 aligned, so `true`-as-any and `oneOf` with `null` are
  legal), emitted as pretty JSON with a trailing newline (readable > terse; matches the SQL/client
  emitters). *Deferred, same as the client + SQL sides:* nested shape sub-objects (`field { … }`)
  are skipped (need JSON aggregation); pure update/delete declared-shape responses ride the `{ id }`
  fallback until D12's re-select extends to them; a YAML rendering (JSON is what generators accept).

## D25 — write-retry idempotency (`based-runtime::idempotency`)
Closes the gap D20 flagged: app-side id-gen (D1) mints a *fresh* `id` per `create`, so a client that
retries a mutation after a `503`/timeout — not knowing if the first attempt committed — would
**double-insert**. An **idempotency key** makes a keyed mutation run its write body **at most once**
per key; a retry replays the first attempt's stored response.
- **Mutations only, opt-in.** A query is naturally idempotent (no writes) and never touches the store.
  No key → the prior run-every-time behaviour. The key is *request metadata* carried out of band on
  the `Idempotency-Key` header, **never the JSON body and never a `$ctx.<field>`** — it is engine
  infrastructure, not application data, so the schema never reads it (same trusted-edge discipline as
  `$ctx`, auth.md/D7). A blank/whitespace header is treated as absent (`Request::with_idempotency_key`).
- **Keyed by `(callable, key)`.** The key is scoped to the callable it accompanies, so one request id
  reused across a batch of different mutations does not collide.
- **Store is a seam** (`IdempotencyStore`, the `Db`/`IdGen` twin). Lifecycle: `begin(callable, key)`
  atomically claims the key or reports it — `Fresh` (claimed → run, then `record` the response, or
  `abandon` on failure so a later retry may re-run), `Done(resp)` (a prior attempt committed → replay
  `resp`, run no writes → exactly-once), `InFlight` (a concurrent attempt holds the key → don't run a
  second write). `MemStore` (a `Mutex`-guarded map) is the in-process impl — correct for a single
  instance and the whole request→response path is testable against it with no infra; `NoStore` is the
  no-op "idempotency off" store so there is **one** dispatch path (P4), not a with/without fork.
- **Wired through the one dispatch core.** `run_mutation` takes `&dyn IdempotencyStore`; **planning
  (arg/`$ctx` validation) runs *before* the store**, so a malformed request is a clean `4xx` that never
  consumes a key (the client fixes it and retries with the *same* key). `dispatch` grows a `store` +
  `idem_key` parameter and maps `RunError::Conflict` (an in-flight duplicate) to a **retryable `409`**
  `idempotency_conflict`. The HTTP edge reads the `Idempotency-Key` header and shares one `MemStore`
  across the worker pool; `embed::Engine::call_with_key` is the in-process twin (key supplied straight
  in, no header). A `Db` fault on a keyed write rolls back *and* abandons the key, so a genuine failure
  stays retryable.
- **Deferred (needs live infra):** a **shared/durable** store so a retry that lands on a *different* app
  instance also dedupes (the `MemStore` dedupes within one process; the trait is identical — back it with
  the DB or a cache in the driver/live-DB slice); **TTL/eviction** of stored keys (they accumulate today);
  binding the key to a request-signature hash so a *replayed key with different args* is rejected rather
  than silently replaying the first (today the key alone is authoritative, the Stripe default).

## D26 — the container story: health/readiness probes + graceful shutdown
Closes what D21/D22/D23 flagged as "still wanted for the standalone container story." `based serve`
now answers the two operational endpoints an orchestrator (Kubernetes) / load balancer expects, and
drains in-flight requests on a shutdown signal — the two pieces that make it a real container, not just
a socket that runs until killed. All of it exercisable over a loopback socket with a mock backend (no
live DB), matching the existing `tests/http.rs` harness.
- **Two probes, answered before routing** (`http::build_response`). They are unauthenticated `GET`s,
  intercepted ahead of `preflight` (which rejects non-POST), so the RPC wire's POST-only rule
  (calling.md) is unchanged — the probes sit *outside* it.
  - **`GET /healthz` = liveness** — "the process is up." Always `200` while a worker can answer; it
    **never touches the backend**, because a DB outage must *drain* an app container (readiness), not
    *restart* it (liveness). A container that fails liveness is killed and replaced.
  - **`GET /readyz` = readiness** — "send me traffic now." `200` only when (a) not draining *and* (b)
    the backend can serve (`Backend::ping`); else a `503` `{ error: { code, message } }`, on which the
    LB pulls the instance out of rotation. Two distinct 503 codes: `draining` (shutdown) and
    `not_ready` (a shard's pool unreachable). Distinct from liveness so a transient DB blip drains, not
    restarts.
- **`Backend::ping`** is the readiness seam (a defaulted trait method — a read-only/mock backend is
  trivially ready). The `ShardRouter` override probes **every** physical shard with a lightweight
  `SELECT 1` (catching a stale pooled connection, not just a checkout): a single down shard makes the
  whole instance report not-ready, because a partial-outage instance is worse than one fewer healthy
  instance — the LB drains it and its shard's traffic fails over with the instance out of the way.
- **Graceful shutdown** via `Handle::shutdown` (returned by the new `serve_with_handle`; the old
  `serve` is now a thin wrapper that discards the handle and runs until killed). It flips a shared
  `AtomicBool` *draining* flag and `unblock()`s the server:
  - Readiness fails **first** (the drain half of a zero-downtime rollout — the LB stops sending new
    requests while in-flight ones finish).
  - Workers poll the flag between requests via `recv_timeout(100ms)` (so a worker blocked on `recv`
    still wakes to observe it), stop accepting new work, and exit **after** their current request
    completes — **no in-flight request is ever cut off** (the drain guarantee). `serve_with_handle`
    then returns once every worker has joined, so the process can exit cleanly.
- **The signal→drain wiring lives in the CLI, not the runtime library** (separation of concerns —
  the library stays signal-free and just exposes `Handle`). `based serve` installs a SIGTERM/SIGINT
  handler (via `ctrlc`, a small hardened crate — principle 7, don't hand-roll signal handling) that
  calls `handle.shutdown()`; a failure to install it is non-fatal (the server still runs, it just
  can't drain — a hard kill still stops it).
- *Deferred:* a container image / Dockerfile (a packaging concern, not code); a shutdown **grace
  deadline** (force-exit after N seconds if a request hangs — today the drain waits indefinitely for
  in-flight requests, correct for the short-request RPC workload, D20); readiness `ping` result caching
  (each `/readyz` runs a live `SELECT 1` per shard — fine at probe frequency, but a high-frequency
  probe against many shards would want a short TTL); the **live-DB hardening** (D20 gap) is still what
  makes `Backend::ping` production-real, not just compile-verified.

## D27 — the SQLite backend (`based-runtime::sqlite`): infra-free real integration
Every runtime test to now drives the plan → run → shape path against a `MockDb` (canned rows), so it
proves *binding* but never that the emitted SQL *executes*. A **SQLite** backend closes that gap with
**no live infra** — bundled in-memory SQLite (via `rusqlite`, feature `sqlite`) is the first concrete
`Db`/`Backend` a test can run the runtime's real read/write SQL through. It is also the lowest-friction
second dialect (D21): SQLite binds positional `?` exactly like MariaDB, so the `?`-vs-`$n` scanner
coupling D21 flagged does **not** apply (that is Postgres-only).
- **`SqliteDb` = `Db` over one shared connection.** `SqlValue`↔`rusqlite::Value` maps family-for-family
  like the MariaDB driver (D20): `bool`→integer `0/1`, `json`→serialized text (SQLite has no JSON
  type — stored as `TEXT`, read back as the wire string), a binary `BLOB`→lowercase hex (never a panic,
  mirroring `from_mysql`). `fetch` builds each row from its column aliases (a row is already the response
  object); `execute`/`begin`/`commit`/`rollback` run the write path. The mapping is pure + unit-tested.
- **`SqliteBackend` = `Backend`, one shared connection, no shards.** SQLite doesn't shard, so it ignores
  the shard key and hands every checkout the *same* connection (a clone of one `Arc<Mutex<Connection>>`).
  Sharing one connection is load-bearing for an **in-memory** DB (it is per-connection — separate
  connections would each see an empty DB), and it is what lets one request's write be visible to the
  next: the property that makes it a genuine integration engine. The `Mutex` gives `Send + Sync` (the
  worker-pool bound) by serializing checkouts — correct for SQLite (a file DB serializes writers anyway);
  a throughput-hungry deployment uses MariaDB's scale-out (D20), not many SQLite connections. `ping`
  runs `SELECT 1` (the D26 readiness seam, now exercised against a real engine, not compile-verified).
- **Real end-to-end tests** (`tests/sqlite_integration.rs`): load the *actual* commerce schema
  (`Compiled::load`) and dispatch real requests through `serve::dispatch` against a live `SqliteDb`,
  running the **verbatim** codegen-lowered SQL (`based gen sql`). Covers a `get` (join + project) + its
  miss → `null`, a `$ctx`-scoped `list` (the injected scope predicate actually filters other orgs out),
  `place_order` (INSERT + declared-shape re-select under one tx, read-your-writes confirmed by a
  follow-up read seeing the new row), a boundary `400`, and `Backend::ping`. No infra → runs in CI like
  any unit test.
- ~~**Deferred: SQLite DDL codegen.**~~ ✅ **done (D28).** `based gen sql` now targets SQLite too —
  the `Dialect` enum grows its **first second variant** (`Dialect::Sqlite`), and the SQLite integration
  test creates its tables from the *generated* DDL rather than a hand-shaped copy.

## D28 — SQLite DDL codegen (`Dialect::Sqlite`)
`based gen sql` can now target SQLite, closing the gap D27 flagged: the SQLite *runtime* backend already
existed, but the *DDL* only emitted MariaDB syntax, so the D27 integration test hand-shaped its setup
schema. `Dialect::Sqlite` is the enum's first second variant (the seam D21 anticipated) and drives
`sql::ddl`; only the DDL branches — the DML/mutation SQL is already dialect-portable (D27), so those
emitters vary only their header comment via the new `Dialect::name()`.
- **Manifest `dialect = "sqlite"`** parses to `Dialect::Sqlite` (`Dialect::parse`); unknown values still
  fall back to MariaDB (the documented default, unchanged).
- **Type map** (module table in `sql.rs`) mirrors the runtime `SqliteDb`↔`SqlValue` mapping (D27) so the
  physical column shape matches what the driver reads/writes: `text`/`uuid`/`Id`/`timestamp`/`date`/`json`
  → `TEXT` (SQLite has no VARCHAR-length/UUID/date/JSON types), `int`/`bool` → `INTEGER` (SQLite `INTEGER`
  is 64-bit; a bool stores `0`/`1`). A to-many scalar → `TEXT` (a JSON string, SQLite having no JSON type).
- **Indexes are separate `CREATE INDEX` statements**, not inline table clauses — SQLite has no inline
  `KEY`/`UNIQUE KEY` syntax. Both the declared `@index`es and the sema-inferred join-key baseline (D15)
  trail the `CREATE TABLE` as `CREATE INDEX` / `CREATE UNIQUE INDEX`, keyed by the *same* `inf_`/`idx_`/
  `uq_` names and physical columns MariaDB uses inline (one `index_specs` helper feeds both dialects, so
  the two can't drift). Column-level `(unique)` constraints stay inline in both (SQLite accepts
  `CONSTRAINT … UNIQUE (…)`). The inferred index stays predicate-leading (soft-delete column first) —
  SQLite has no partial indexes either.
- **Bool defaults** render `1`/`0` on SQLite (integer storage) vs. `TRUE`/`FALSE` on MariaDB; string/int/
  `now()`→`CURRENT_TIMESTAMP` defaults are identical. `PRIMARY KEY (\`id\`)` and the no-FK-constraint rule
  (relations.md) are unchanged.
- **Proof it runs.** `based-runtime/tests/sqlite_integration.rs` (D27) now creates its tables from
  `sql::ddl(&schema, Dialect::Sqlite)` — the verbatim `based gen sql` DDL — instead of a hand-shaped copy,
  so the whole `based gen sql` artifact (DDL *and* DML) is proven to execute end-to-end against a real
  engine. Tests: 8 new SQLite cases in `based-codegen/tests/ddl.rs` (type map, `TEXT` id/FK, integer bool
  default, inline `(unique)`, indexes-as-`CREATE INDEX` after the table, inferred join key).
- ~~*Deferred:* Postgres remains the outstanding dialect~~ ✅ **done (D29).** `Dialect::Postgres` is the
  enum's third variant — DDL + DML + mutation codegen *and* the dialect-aware named→positional scanner
  (`?` → `$n`, the one coupling D21 flagged). Per-field `text` length tuning is still unaddressed (no
  length primitive; D10) but is moot on SQLite/Postgres (untyped/unbounded `TEXT`).

## D29 — Postgres dialect (`Dialect::Postgres`): codegen + the `$n` scanner
`based gen sql` can now target PostgreSQL, and the runtime binds it. This is the dialect that actually
*exercises* the multi-dialect seam D21 built for — Postgres diverges from MySQL/MariaDB the most, so it
forced the codegen quoting/operator differences and the one runtime coupling (`?` → `$n`) that SQLite
(D27/D28) did not. The concrete `postgres` `Db`/`Backend` **driver** is deferred to the live-DB slice
(needs a real server to be meaningful, like `MariaDb`'s connect/exec, which is compile-verified only);
this D is the *codegen* + scanner half, fully `cargo test`-verifiable.
- **Identifier quoting is the pervasive difference.** MySQL/MariaDB and SQLite backtick-quote
  identifiers; Postgres uses ANSI double quotes (`"order"` — and `order` is a reserved word, so quoting
  is load-bearing). Rather than hardcode the quote char at ~40 `format!` sites, it is routed through
  `Dialect::quote`/`qcol` (and a `Select::q`/`qcol` for the DML/mutation hub). The DDL emitters thread
  the same helper. So "dialect-portable" (the old D27 claim for DML) is now false — the DML/mutation SQL
  *does* branch, just through one quoting seam plus a few operator/literal spellings.
- **Type map** (`sql::sql_type`): `int`→`BIGINT`, `bool`→`BOOLEAN`, `timestamp`→`TIMESTAMPTZ` (a real
  tz-aware type — no `DATETIME` 2038/ON-UPDATE dodge needed), `date`→`DATE`, `text`→`TEXT` (unbounded,
  no VARCHAR cap), `uuid`/`Id`→`UUID` (native), `json`/a to-many scalar→`JSONB` (the indexable,
  `@>`-queryable form — matching the DML `has` lowering). Bool defaults use the `TRUE`/`FALSE` keyword
  (like MariaDB, unlike SQLite's `1`/`0`).
- **Indexes as separate `CREATE INDEX`** (like SQLite — Postgres has no inline `KEY`/`UNIQUE KEY`
  clause). The declared `@index`es and the sema-inferred join-key baseline (D15) trail the `CREATE
  TABLE`, keyed by the same `inf_`/`idx_`/`uq_` names + physical columns MariaDB inlines (one
  `index_specs` helper feeds all three dialects). `(unique)` stays an inline `CONSTRAINT … UNIQUE` in
  all three. Inferred indexes stay predicate-leading (soft-delete column first — Postgres partial
  indexes exist but are not used here, keeping the DDL uniform with the other dialects).
- **Operators.** `has` (JSON-array containment) → Postgres's `arr @> value` (JSONB containment), not
  MySQL's `value MEMBER OF(arr)`. `= TRUE` for a bare bool (Postgres has the keyword). `IS NULL`,
  `LIKE`, comparison ops, `IN (…)` are shared.
- **Multi-table UPDATE/DELETE restructure.** MySQL puts joins inline (`UPDATE t JOIN j ON … SET …`;
  `DELETE t FROM t JOIN j …`). Postgres has no inline join in a write: the joined tables move to a
  `FROM` (UPDATE) / `USING` (DELETE) list and each join `ON` folds into the `WHERE` ahead of the user
  predicate (`push_from_using`). A `LEFT JOIN`'s outer semantics are lost in this fold, but a mutation
  `where` only *narrows* the target set (it never projects the joined row), so an inner join is the
  correct — and only expressible — shape. Postgres also **forbids the target alias in `SET`**, so a SET
  column is emitted bare (`"col" = …`, via `set_lhs`) there while MySQL/SQLite qualify it.
- **The scanner is now dialect-aware** (`based-runtime::scan::to_positional`, the D21 coupling):
  `:name` → `?` for MySQL/MariaDB/SQLite, `:name` → ordinal `$1, $2, …` for Postgres (the running
  parameter count, so it matches bind order). `::` is skipped whole (Postgres's cast operator, not a
  placeholder). `Compiled` now carries the `Dialect` (read from the manifest in `load`, passed to
  `from_checked`) and threads it to the `Env` that binds — so a Postgres `Compiled` lowers double-quoted
  SQL *and* binds `$n`, and the pairing "Postgres-lowered SQL served only to a Postgres backend" (D21's
  coherence rule) is one field, not a negotiation.
- **Tests** (all `cargo test`, no infra): 2 new in `Dialect` unit (`parse`/`quote`/`bool_lit`), 4 DDL
  (double-quoting + native types, UUID FK, keyword bool default, separate `CREATE INDEX`), 4 DML
  (double-quoted SELECT + `:name` retained, bare-bool `= TRUE`, `has` → `@>`, join `ON`), 4 mutation
  (double-quoted INSERT + re-select, bare-column tombstone SET, `FROM`-clause update, `USING`-clause
  hard delete), 2 scanner (`$n` ordinals, `::` cast untouched), 2 runtime plan (`$1`/`$2` end-to-end
  binding, incl. offset ordering). The commerce example emits clean Postgres DDL + DML.
- *Deferred:* the concrete `postgres` driver (`Db`/`Backend`) + live-DB integration (needs a real
  server — the D20 gap, same status as `MariaDb`); Postgres-specific niceties not used here (partial
  indexes for soft-delete, `INSERT … ON CONFLICT`, `RETURNING` — the D12 re-select is dialect-portable
  and kept). MySQL stays folded into `MariaDb` (a fork; the emitted SQL is MySQL-8-compatible), not a
  distinct variant.

## D30 — typed per-callable `$ctx` in the generated Rust client (`based gen client`)
Closes the D4/D13 residue "emitting the per-callable `Ctx` type in the client." Until now the generated
Rust client had **no typed surface for `$ctx` at all**: `Transport::call(route, input)` couldn't carry
context, so a caller couldn't see from the generated code what context each callable needs, and the
in-process bridge (`tests/embed.rs`) was forced to smuggle `$ctx` on the side (held on the bridge
struct, per unit-of-work). `$ctx` is per-request and inferred (D4/D5), and the client now mirrors that
inference exactly.
- **One typed `<Name>Ctx` struct per callable that reads context.** Its fields are the callable's
  deduped `ctx_requires` bag (`RQuery`/`RMutation.ctx_requires`, read straight off the IR — no new
  resolution), one field per required `$ctx.<field>`. A field's type follows the *same* inference the
  input side uses (D13): a relation requirement (`CtxField::Relation`) carries the model's key `Uuid`
  (D1); a scalar requirement (`CtxField::Scalar`) is that column's primitive (an `int`-compared
  `$ctx.tier` → `i64`). A callable with **no** `$ctx` requirements emits **no** struct and takes
  `ctx: ()`, so the common public case stays clean (principle 2 — the empty context has one safe
  meaning, and deviation is the visible typed struct).
- **The method carries `$ctx` as a typed argument.** `fn <name>(&self, input: <Name>Input, ctx:
  <Name>Ctx | ())` — the context sits *next to* the input, never merged into it, because `$ctx` is
  request context an upstream sets, not a caller-supplied body field (auth.md/D7). So the generated
  signature is now the honest contract: reading it tells you both the arguments *and* the context the
  callable requires.
- **`Transport` carries the context generically.** `call<I, C, O>(route, input, ctx)` grows a
  `C: Serialize` context parameter alongside `I: Serialize`. The concrete `Transport` serializes `ctx`
  to the same JSON `$ctx` bag the engine already consumes (`Request` context, D18/D21); `&()`
  serializes to JSON `null`, which the bridge maps to an empty bag. One trait shape serves both the
  in-process door (D22 — `$ctx` supplied straight in) and the HTTP door (D21 — the bridge would set the
  `X-Based-Context` header from the serialized context instead). The runtime still owns *how* the
  context reaches the wire; codegen only types *what* it is.
- **Reuses the existing type resolver** (`primitive`/`Uuid`), so the `<Name>Ctx` field types can't
  drift from the input-side field types for the same column (principle 4). No client-side ctx logic
  beyond reading `ctx_requires` + one struct-emission arm — the shape mirrors the sema/facts rendering
  (D4/D5) and the OpenAPI `x-ctx-requires` extension (D24), all fed by the one IR bag.
- **Tests:** 4 new in `based-codegen/tests/client.rs` (14 total) — the transport signature carries
  `ctx`, a `$ctx.org` callable gets a `MyOrgOrdersCtx { org: Uuid }` the method takes, a public
  callable takes `()` and emits no `Ctx` struct, and a scalar `$ctx.tier` types as `i64`. `tests/
  embed.rs` (the verbatim generated client over `MockDb`) is regenerated to the new signature and its
  `$ctx` round-trip now supplies a typed `MyOrgOrdersCtx` straight in (no side-channel bag), proving the
  end-to-end typed path; the `embed` module doc's bridge example is updated to the new trait.
- *Deferred:* the HTTP `Transport` bridge that maps the serialized context to the `X-Based-Context`
  header (a runtime concern; the trait shape is ready); a non-Rust client's `$ctx` typing rides D24's
  OpenAPI `x-ctx-requires` vendor extension (descriptive, not a generated struct) rather than this
  Rust-only struct.

## D31 — idempotency-key request fingerprint (`based-runtime::idempotency`)
Closes the D25 residue "rejecting a replayed key carrying *different* args." Until now the idempotency
key alone was authoritative (the Stripe *default*): a caller who reused one `Idempotency-Key` for two
**different** requests silently got the *first* request's response back for the second — a quiet wrong
answer, the worst quadrant of principle 1. The key now also carries a **request fingerprint**, so a
reused key on a changed payload is caught and rejected loudly instead of replayed.
- **The fingerprint is a stable hash of the request payload** — its args + `$ctx`, `Request::fingerprint`.
  FNV-1a over the canonical JSON of the two maps (`serde_json::Map` is `BTreeMap`-backed → sorted keys →
  canonical `to_string`), with a separator byte between them so moving a field from args to `$ctx` changes
  the hash. FNV (not `DefaultHasher`) because it is stable across releases — the durable multi-instance
  store (still deferred, D25) will compare fingerprints minted by a different process. The idempotency key
  and callable are **excluded** from the fingerprint: the store already scopes an entry by `(callable,
  key)`, so the fingerprint's sole job is detecting a *payload* change under a reused `(callable, key)`.
- **`IdempotencyStore::begin` grows a `fingerprint` parameter**, and each stored `Entry` (`InFlight` /
  `Done`) records the fingerprint of the attempt that created it. A `begin` replays (`Done`) or blocks
  (`InFlight`) **only** for a *matching* fingerprint; a different one is the new `KeyState::Mismatch`. The
  matching case is unchanged — a genuine retry carries the same payload, so it still dedupes exactly as
  before (D25). `record` preserves the claiming `begin`'s fingerprint (a stray record with no live claim
  falls back to `Fingerprint::MAX`, which never matches a real request, so it can never be replayed).
  `NoStore` ignores the fingerprint (always `Fresh`), so the "idempotency off" path is unchanged.
- **`run_mutation` computes `req.fingerprint()` and threads it to `begin`**; a `Mismatch` becomes the new
  `RunError::KeyReuse`. `dispatch` maps it to a **non-retryable `422`** `idempotency_key_reuse` (the
  request is well-formed but its key/payload pairing is unprocessable — distinct from the retryable `409`
  `idempotency_conflict` an in-flight *same-payload* duplicate still gets). The client's fix is a fresh
  key for the genuinely different request; retrying the *same* key/payload is not the answer, hence
  non-retryable. No write runs and nothing is replayed on the mismatch.
- **Why not silently replay (the old Stripe default).** Replaying the first result for a *different*
  request answers the wrong question — a correctness bug the caller can't see. Principle 1 (dangerous is
  explicit + visible) and principle 2 (nothing consequential is true by omission) both say make it loud.
  A `422` costs the careless caller a clear error; the careful caller (fresh key per request) is unaffected.
- **Tests** (all `cargo test`, no infra): 2 new store unit tests (a different fingerprint on a `Done` and
  on an `InFlight` key → `Mismatch`, and the original fingerprint still replays afterward — the mismatch
  doesn't corrupt the entry), 1 `Request::fingerprint` unit test (stable per payload, key-invariant,
  order-invariant, args↔ctx move detected), and 1 end-to-end `serve.rs` test (same key + different args →
  `422` with no SQL, and the genuine retry still replays). The existing 4 idempotency tests carry a
  stand-in fingerprint (the exact hash is opaque — only equality matters).
- *Deferred (unchanged from D25):* the shared/durable multi-instance store (needs live infra; the seam +
  the stable FNV fingerprint are now ready for it); key TTL/eviction.

## D32 — `@scope` resolved: uniform single-owner row filter + create auto-set + `unscoped`
Resolves D19 (whose four axes are settled here). `@scope` is a **model-level, uniform,
single-owner row-visibility filter** — a standing predicate parameterized by request context
(auth.md Handle 2). It is *not* an authorization model, not per-operation, and not a policy
engine. Injection (read `WHERE` / write `WHERE` / joined `ON` / `$ctx` propagation) was already
built when this landed; D32 closes the three gaps that made it *safe*.

- **Predicate is restricted to a conjunction of `col = $ctx.field` equalities** (`E0180`,
  `resolve::check_scope_form`). `col` is a single-segment column or forward-FK on the model; the
  RHS is strictly `$ctx.<field>`. No `or` / `in` / range / literal-RHS / multi-hop path / named
  filter. This is what makes scope injectable *everywhere* and — critically — auto-settable on
  `create`; a non-equality/multi-owner rule has no create-time projection and is not a scope. D19
  already said multi-owner (`org in $ctx.writable_orgs`) is Handle 1, not scope — the restriction
  makes that structural. `RModel::scope_terms()` flattens the predicate to `(field, ctx_field)`
  pairs for reuse (create auto-set today; shard-key binding later).
- **Create-time is closed by auto-set** (the safety headline). On a scoped model the scope column
  is **engine-managed on `create`**: codegen injects `<col> = :ctx_<field>` into the INSERT
  (`sql::mutations::lower_create`), so the row always lands in the caller's own scope and a
  **cross-scope create is inexpressible**. A caller that *assigns* the scope column is `E0181`
  (`check::check_scope_assign`); the column is required-exempt (`E0146`) like `id`/`@created`; and
  the create now *requires* the scope `$ctx` field (`ctx::scope_ctx` on the Create arm), so the
  client sends it. Before D32 an out-of-scope `create` silently inserted the row and only the
  re-select hid it — a principle-1 violation, now removed.
- **`unscoped("reason")` is the escape hatch** (principle 6 — mandatory, minimal-scope, greppable,
  linted). A per-callable clause (grammar `unscoped_clause`, after the return type on a query /
  after any `guard` on a mutation), carrying a **mandatory reason string** (never silent). It opts
  the *one* callable out of *all* scope handling — read/write injection *and* the create auto-set —
  for cross-scope access (admin/support/jobs/import). It forfeits **only** `@scope`; soft-delete
  still applies. `W0106` flags a stale `unscoped` (target has no `@scope`). Threaded through sema
  (`ctx` drops the scope requirement), dml, and every mutation-write lowering.
- **What the compiler guarantees:** the predicate is injected into every read/write on the model
  except explicit `unscoped` sites; cross-scope create is inexpressible; every opt-out is
  greppable + reason-carrying + lintable. **What it does not:** verify the predicate is the correct
  authz rule, or evaluate any role/permission matrix (that is Handle 3 `guard`, host-language).
  `@scope` is a row-visibility filter, not a checked authorization model — do not mistake it for one.
- **Commerce demonstration:** `Order` is now `@scope(org = $ctx.org)`, so every order query is
  org-scoped from `$ctx`; `place_order` dropped its `org` param (auto-set); `orders_in_org` is the
  admin cross-org lookup, marked `unscoped("admin: cross-org order lookup")`; `my_org_orders` is a
  plain `list Order` (scope does the filtering).
- **Tests:** 8 new sema (`E0180` × 3 forms, multi-term clean, `E0181`, create-exempt clean +
  ctx bag, `unscoped` drops ctx, unscoped-create-may-assign, `W0106`), 3 codegen (dml unscoped
  omits scope; mutations create auto-set + re-select scope; unscoped mutation/update omit), 1
  parser (unscoped on query + mutation), conformance `ctx_scope` re-blessed to the new pattern.
- ~~**Follow-on (separate slice):** bind D20's shard key to the scope field~~ ✅ **done (D33).**
  ~~Also deferred: injecting `@scope` into a *joined* table's `ON`~~ ✅ **done (D34):** a query/mutation
  reaching another scoped model through a relation now carries that model's `@scope` into the join
  `ON` (the same slot soft-delete uses), closing the cross-scope leak on the join side.

## D33 — shard key bound to the resolved `@scope` `$ctx` field
Closes the follow-on D32 flagged and the shard-key hole D20 left open. D20 built the `ShardRouter`
(one bounded pool per physical shard, a stable FNV logical-shard hash) but left the **key source**
pluggable and unbound — the natural key is the tenant/owner, i.e. the field `@scope` uses, but
`@scope` was OPEN (D19). D32 resolved `@scope` to a single `col = $ctx.field` equality, so the
owner field is now unambiguous. D33 makes the shard key that field, derived from the schema per
callable — retiring the hand-set `--shard-key-field` config.
- **The shard key is the callable's target model's `@scope` owner field** ([`RModel::
  shard_key_ctx_field`] → the `$ctx` field of the first scope term; `@scope(org = $ctx.org)` →
  `org`). Read off the *same* `@scope` that filters the rows (D32), so the shard a row lives in and
  the shard its owner's requests route to share one source of truth (principle 4) — they can never
  drift, which a separate `--shard-key-field` flag could. A multi-term scope shards on its first
  `$ctx` field (the rest narrow *within* that owner's shard).
- **Resolved in sema, carried on the IR.** `RQuery::shard_key` / `RMutation::shard_key`
  (`Option<String>`) record the field, computed where `@scope` *and* `unscoped` are both visible.
  An `unscoped` callable (D32) is `None`: it deliberately reads/writes across scopes, so it has no
  single owning shard and must route by an explicit key (never by a scope it disabled — the safe,
  loud default, principle 1). A mutation routes on its **return model**'s scope field: a `tx` is a
  single-shard unit (D20 — no distributed transaction), so the primary written model's owner is the
  one shard.
- **The listener derives the key from the route + `$ctx`** (`http::resolve_shard_key`, pure +
  unit-tested): `Compiled::shard_key_field(is_mutation, name)` gives the callable's scope field, and
  the listener pulls that field's value out of the request `$ctx` (server-supplied, never the body —
  auth.md/D7). Precedence: an explicit `X-Based-Shard-Key` **override** wins (the escape hatch for a
  deployment that must route otherwise, or a callable with no `@scope`), else the schema-derived
  field, else `""` (an `unscoped` callable / an unscoped model / a single-shard deployment → shard
  0). A non-string owner (an int tenant id) is stringified so the FNV hash sees a stable byte string;
  a scoped callable whose `$ctx` lacks the field routes to `""` (the missing-`$ctx` `400` follows in
  `dispatch` — routing only picks the shard).
- **`ContextSource` no longer produces the key.** `Context` now carries `$ctx` + an *optional*
  `shard_key_override` (the header), not a resolved key; `TrustedHeaderContext` dropped its
  `shard_key_field` config (a unit struct now), and the CLI dropped the `--shard-key-field` flag —
  the schema is authoritative. The `serve::route_target(path)` helper exposes the route grammar so
  the edge resolves the callable (for its shard key) *before* checkout, using the same grammar
  `dispatch` enforces.
- **What the compiler guarantees:** every request routes on the field its model is scoped by, with
  no per-deployment config to keep in step. **What it does not:** verify a single `tx` writes only
  same-shard models (a cross-shard `tx` is a deployment invariant, D20 — the return model's field is
  the well-defined key); the concrete driver still needs the live-DB slice to exercise real routing.
- **Tests:** 9 new (`based-runtime/src/http.rs`) — a scoped query and a scoped mutation each shard on
  their scope `$ctx` field, an `unscoped` callable and an unscoped model each have `None`/`""`, the
  explicit header overrides a scoped callable, a non-string owner is stringified, a missing `$ctx`
  value routes to `""`; the two `TrustedHeaderContext` tests updated to the override shape.
- *Deferred:* binding the *concrete* driver's shard routing end-to-end (the live-DB slice, D20/D29);
  a `tx` that spans two differently-scoped models is unmodelled (single-shard invariant).

## D34 — `@scope` injected into a *joined* table's `ON` (closes the D32 follow-on)
Closes the cross-scope leak D32 explicitly left open: until now `@scope` was injected only into the
**root** table's `WHERE` (reads) and the **write-target**'s `WHERE`, so a query that reached a
*different* scoped model **through a relation** read that joined model **unfiltered** — a
tenant-boundary leak on the join side. Soft-delete already injected its tombstone into *every* joined
table's `ON` (D11); D34 makes `@scope` ride the *exact same slot*, so a relation reach can no longer
read across a scope boundary.
- **Injection point = the join resolver, not the caller.** `Select::join_forward`/`join_inverse`
  (`based-codegen::sql::dml`) already append the joined model's soft-delete predicate to the `ON`;
  they now also append its `@scope` (via `Select::scope_join_pred`). Every join — from a `where`
  path, a sort path (query `order` or model `@sort`), or a return-shape `out = path` reach — flows
  through this one chokepoint, so the injection is uniform across all join sources with no per-source
  code. A `LEFT JOIN` stays a left join (the predicate is in `ON`, not `WHERE`): an out-of-scope
  joined row simply yields NULLs; an `INNER JOIN`'s row drops entirely (the correct, only expressible
  shape — a required relation to an out-of-scope owner has no in-scope match).
- **The bind is the *same* `:ctx_<field>`** the root scope + create auto-set use (D11/D32). A D32
  scope term `col = $ctx.field` on the joined model becomes `<join_alias>.<physical_col> = :ctx_<field>`.
  The runtime binds `:ctx_<field>` **once** from the request `$ctx`, so no new bind surface is added
  (principle 4) — every scoped table in the query, root or joined, reads the same context value.
  Closed-world coherence (D4) guarantees a `$ctx` field name means one type everywhere, so sharing the
  bind across tables is sound.
- **Sema makes the callable *require* the joined field** (`based-sema::ctx`, D4/D5). Because codegen
  now emits `:ctx_<field>` for a joined scope, the callable **must** carry that field in its
  `ctx_requires` bag or the bind is unbound at runtime. `collect_query`/`collect_mutation` gained a
  joined-scope walk that mirrors codegen's joins: it follows each relation reach in a `where` path,
  the sort path, and the return shape's `out = path` reaches (a `Nest { … }` sub-object is **not**
  walked — codegen defers those, so they produce no join, and sema stays aligned) and records the
  `@scope` `$ctx` of every scoped model traversed *into* (the terminal segment is a column, not a
  join, so it is skipped). A mutation additionally walks its write `where`s and its declared-shape
  re-select (D12). The scope's field type is inferred by resolving the joined model's `@scope`
  against that model — reusing the existing `walk_pred` inference, so the joined-field type can't
  drift from the root-field type for the same column (P4). `Cx` grew a `shape_bodies` map (shape name
  → body) so the collector can reach a return shape's reaches.
- **`unscoped` (D32) drops the joins too.** An `unscoped` callable opts out of *all* scope handling —
  the joined tables' `@scope` as well as the root's — in one decision: codegen passes
  `Select::with_scope_inject(false)` (so `scope_join_pred` returns nothing) and sema collects no
  joined-scope requirement. Root and join opt-out can't diverge (they read the one `unscoped` flag).
- **What the compiler guarantees:** a scoped model is filtered by its `@scope` *wherever it appears*
  in a query — as the root, a write target, **or a joined relation** — except at explicit `unscoped`
  sites. **What it does not** (unchanged from D32): verify the predicate is the correct authz rule.
- **Commerce is unchanged** (its only scoped model, `Order`, is reached-*from*, never reached-*into*
  — you don't scope the tenant-owner table by itself), so all goldens are byte-identical; the new
  behaviour is proven on a synthetic `Ticket → Contact` (`@scope(org = $ctx.org)`) topology instead.
- **Tests:** 3 codegen (`based-codegen/tests/dml.rs`: shape-reach + `where`-reach inject
  `contact.org_id = :ctx_org` into the join `ON`; `unscoped` injects none), 4 sema
  (`based-sema/tests/check.rs`: a shape-reach and a `where`-reach each add the joined `org` to
  `ctx_requires`, a mutation's re-select does too, `unscoped` drops it), and 1 **real end-to-end**
  (`based-runtime/tests/sqlite_integration.rs`): a `Ticket → Contact?` cross-org row read against a
  live SQLite engine comes back with the joined `who` NULL for an out-of-scope caller and populated
  for the in-scope one — the leak is actually closed, not just bound.
- *Deferred:* injecting `@scope` into a joined table reached through a *named-filter* body's own
  relation reach when the filter expands to a differently-scoped model (the filter-call path resolves
  columns but the joined-scope walk doesn't recurse filter *bodies* for joins yet — rare; direct
  reaches, the common case, are covered); a joined model with a *multi-term* scope injects all its
  terms (handled — `scope_terms()` is flattened), but a term whose `$ctx` field is used **only** on a
  join (never on the root) still relies on the runtime binding it (it does — it's in `ctx_requires`).

## D35 — Docker-backed real-DB integration harness + the MariaDB live suite (Track A1+A2)
Turns "architecture-ready" into "proven" for the `MariaDb` driver (D20), which until now was only
**compile-verified** — its connect/exec paths never ran against a real server. Two pieces, both behind
the new `docker-tests` cargo feature (off by default; the core stays infra-free):
- **The harness** (`crates/based-runtime/tests/support/docker_mariadb.rs`). `MariaDbContainer::start()`
  shells out to the `docker` CLI to `docker run --rm --detach` a pinned **`mariadb:11.4`** on a random
  free host port (`-p 0:3306`, read back via `docker port`), polls a *real* connection (`SELECT 1`)
  until ready, and force-removes the container on `Drop` — so even a panicking test cleans up. Chosen
  over testcontainers-rs deliberately: testcontainers pulls an **async runtime**, and this codebase is
  sync-by-decision (D20); a thin `docker run` guard reuses the hardened external tool (principle 7)
  without importing tokio into the test tree.
- **Skip-never-fail** (the load-bearing property). When the Docker daemon is unreachable (`docker info`
  exits non-zero — a fast, reliable probe) `start()` returns `None` after logging a clear
  `[docker-mariadb] SKIP: …` line; each test early-returns on `None`. So `cargo test --workspace
  --all-features` is **green with or without a daemon** — the real-DB proof runs when infra is present
  and is simply absent otherwise, never turning missing infra into a red build (principle 1: the safe
  state is the silent default; the dangerous "no proof ran" case is a visible log, not a hidden pass).
- **The live suite** (`crates/based-runtime/tests/mariadb_integration.rs`, 7 tests — the MariaDB twin of
  `sqlite_integration.rs`, D27). It loads the *actual* commerce schema (`Compiled::load`, whose manifest
  dialect is `mariadb`, so the DML lowers with `?` binds — exactly what this driver runs), creates every
  table from the **generated** MariaDB DDL (`sql::ddl(_, Dialect::MariaDb)`, not a hand copy — so the
  whole `based gen sql` artifact, DDL *and* DML, is proven to execute), seeds fixtures, and drives real
  requests through `serve::dispatch` against a concrete `MariaDb` checked out of a live
  `ShardRouter::single`. Coverage (DoD #1): a `get` (join + project) + its miss→`null`, a `$ctx`-scoped
  `list` (row scope actually filters; a different org sees nothing) + the joined-`ON` reach projecting
  live, the `place_order` write (INSERT + declared-shape re-select under one tx, read-your-writes
  verified by a follow-up read), idempotency-key dedupe (a retry replays, no double-insert), and
  `Backend::ping`. **Ran genuinely green against a live MariaDB 11.4.**
- **`docker-tests` enables `serve`, not just `mariadb`.** The generated MariaDB DDL emits **native `UUID`
  columns**, which *validate* on insert/compare — so the deterministic `SeqIdGen` (`id-0`, …) and
  `'org-1'`-style fixtures the SQLite test uses are rejected. The suite therefore uses the production
  `UuidGen` (v4 uuids, valid for a `UUID` column — the same generator prod uses, gated on `serve`) for
  engine ids, and valid-UUID literals for the seed rows. A `get`/`list` *miss* can still pass a non-UUID
  string like `'nope'` — MariaDB treats an invalid-UUID comparison value as simply non-matching (empty
  result), not an error (verified), so the miss cases read naturally.
- **What this proves / unblocks:** the `MariaDb` `Db`/`Backend`/`ping` seams work against a real server,
  not on paper; the generated DDL executes on MariaDB; and the A1 harness is now the reusable seam the
  **Postgres** driver + live suite (Track A3) plug straight into. It is the completion roadmap's
  largest-leverage first step (DoD #1).
- *Deferred (Track A4, live-DB hardening):* typed JSON reconstruction for `JSON` columns, statement
  timeouts, deadlock-retry, pool-exhaustion → 503 under load — designed (D20/D26) but not yet
  stress-proven against the live server; a shared/durable idempotency store for multi-instance dedupe
  (D25). A Postgres live suite awaits its concrete driver (A3).

## D36 — VS Code extension: a thin LSP client under `editors/vscode/` (DoD #3, Track C)
Turns "the server speaks standard LSP" (M5) into "a human can install an extension and get live
feedback." The extension is deliberately **thin**: all intelligence stays in `based-lsp` (principle
4 — one source of truth), and the client is only the transport that launches it and registers the
`bsl` language.

- **Separate toolchain, separate tree.** `editors/vscode/` is a TypeScript/npm project **outside** the
  cargo workspace — `cargo` is entirely unaffected. Its gate is `tsc` + `@vscode/vsce package`, not
  `cargo test`. This keeps Track C independent of the Rust driver work (Track A) so the two can run in
  parallel without shared files.
- **What it contributes.** `package.json` registers the `bsl` language for `.bsl`, a minimal TextMate
  grammar (`syntaxes/bsl.tmLanguage.json` — comments, strings, raw backtick blocks, decorators,
  keyword-ish, `$params`; deliberately not exhaustive) and a `language-configuration.json` (comment =
  `#`, brackets), and one setting `basedls.serverPath` (defaults to `based-lsp` on PATH; the user
  builds it with `cargo build -p based-lsp` and points the setting at `target/debug/based-lsp` if it
  is not on PATH), plus `basedls.trace.server` for wire tracing.
- **The client** (`src/extension.ts`, `vscode-languageclient/node`) launches `based-lsp` over
  **stdio** (`TransportKind.stdio`, no args — the server globs `**/*.bsl` from the workspace root it
  gets at `initialize`) and attaches it to `{ language: "bsl" }`. Diagnostics are published unprompted
  by the server; **inlay hints** + **hover** are negotiated automatically from the capabilities the
  server already advertises (M5), so no extra client-side enabling is needed beyond registering the
  language. Startup failure surfaces a clear "build it / set serverPath" error message.
- **Packaging.** `npm run compile` (`tsc` → `out/extension.js`) then `npm run package` /
  `npx @vscode/vsce package` → `based-vscode-<version>.vsix`, installable with
  `code --install-extension`. The `.vscodeignore` ships only `out/**` + the manifest/grammar/config +
  README; `src/`, `node_modules/`, and maps are excluded.
- **Not done (server-side, deferred):** go-to-definition / completion / rename — the server doesn't
  serve them yet (needs the position→symbol layer M5 flags), so the client can't surface them. When the
  server grows them, the client picks them up for free (same capability-negotiation path).

## D37 — migration generation (Track E, spec'd first)
Pointer decision — the full design lives in **PLAN Track E** (settled 2026-07-06) and is now written
up as prose in **`spec/syntax/migrations.md`** (E1). The two commitments worth pinning here (they are
parser/AST/artifact shapes an implementer needs, not naturally prose):
- **`@was("old_name")` is dual-form** (migrations.md): a field-level **`modifier`** (grammar
  `was_directive`, after `modifiers`/`relation_opts`, before `@sort`) declaring a column's previous
  physical name, *and* a model-level **decorator** (`@was("old_table")`, matches the generic
  `decorator` rule) for a table rename. It is a **diff-time** directive — consumed by `based migrate
  gen` to emit a clean `rename` step, then spent (a stale `@was` is a lint, E5). No `@was` ⇒ drop+add,
  never an auto-guessed rename (principle 2). It is the *only* new authored `.bsl` surface migrations
  add; the migration files (`up.mig`/`down.mig`/`schema.snap`) are generated artifacts, not authored,
  so they get no `.bsl` grammar.
- **Artifact layout is fixed:** `migrations/NNNN_slug/{up.mig, schema.snap[, down.mig]}`, zero-padded
  gap-free sequential order, latest `schema.snap` = the diff baseline; the `_based_migrations` ledger
  (id, content_hash, applied_at) with the edited-after-applied ⇒ hard-error tamper rule. Rationale +
  the neutral-step vocabulary + the `raw(dialect)` "not offline-verifiable" contract are in the spec;
  open sub-details (snapshot serialization grammar, raw-step structural-effect annotation, hash
  canonicalization, down-invocation surface) are flagged as TODOs there for E2–E5.

## D38 — the concrete Postgres driver + its live suite (Track A3)
The Postgres runtime driver (`based-runtime::postgres`, feature `postgres`) — the twin of the
MariaDB driver (D20) — plus its Docker-backed live integration suite. Postgres *codegen*
(`ddl`/`dml`/`mutations`) and the dialect-aware `:name`→`$n` scanner were already done (D29); this
is the runtime that *runs* that emitted SQL against a real server. Coverage-wise it closes DoD #1
for the last target dialect (SQLite ✅ D27, MariaDB ✅ D35, Postgres ✅ here).
- **Structure mirrors MariaDB exactly.** `PostgresDb` is a `Db` over one pooled connection (whole
  request on one connection — a `tx` must see its own writes); `PgRouter` is the `ShardRouter` twin
  (one bounded pool per physical shard, single-shard dispatch by the same stable FNV logical-shard
  hash — no scatter-gather, a `tx` is one shard, no distributed transaction). The shared routing
  primitives (`fnv1a_64`, `LOGICAL_SHARDS`, `PoolConfig`, `ShardId`) moved to a new backend-agnostic
  `crate::shard` module so a key routes **identically** regardless of dialect (re-exported from
  `driver` for the historical `based_runtime::driver::{PoolConfig, ShardId}` paths).
- **Reuse, sync, TLS off (principle 7, D20).** The pure-Rust **synchronous** `postgres` crate — *not*
  its async `tokio-postgres` sibling — matches D20's sync/bounded-pool model (no async runtime pulled
  into the codebase). The bounded pool is `r2d2` + `r2d2_postgres` (the crate's own pool is the async
  one). TLS is off (`default-features = false`, `NoTls`) to avoid a system OpenSSL dependency, exactly
  the choice the `mysql` driver made — a deployment needing in-transit encryption re-enables it.
- **The value-mapping crux (the one genuinely Postgres-specific decision).** The runtime is
  dialect-neutral: a `uuid`/`timestamptz`/`jsonb`/`date` value is carried as `SqlValue::Text` (a
  String — on the wire these are all strings, D1). Unlike MySQL/SQLite, Postgres's extended-query
  protocol *infers* each `$n` parameter's OID from the column it binds against and, in **binary**
  format, refuses a text-encoded Rust `String` for an inferred `uuid`/`jsonb` OID. The fix is a
  `PgValue` `ToSql` newtype that (a) `accepts` any OID and (b) reports `Format::Text` for its string
  variant, writing the raw UTF-8 bytes — so the server applies its normal **string-literal coercion**
  (the identical path `'…'::uuid` / `'…'::jsonb` takes). Numbers/bool keep native binary encoding;
  `Null` reports `IsNull::Yes` regardless of the inferred type. This keeps the runtime free of any
  per-column Postgres type table while round-tripping every family. *Rejected alternatives:* rewriting
  the generated SQL to add `$n::text` casts (the SQL is codegen-produced, D18/P4 — the runtime must not
  re-author it); carrying per-column Postgres types into the runtime (a second source of column typing
  that would drift from the schema, against P4). Reads go the symmetric way — every non-numeric/bool
  column is pulled out as its text representation via a `FromSql` that accepts any OID, so uuid/
  timestamptz/date/json all ride back as JSON strings (matching `from_mysql`); a genuinely binary
  column falls back to hex (never a panic). The mapping is pure and unit-tested like `from_mysql`.
- **The live suite is the real gate.** `tests/postgres_integration.rs` (7 tests, the Postgres twin of
  `mariadb_integration.rs`) over a new harness `tests/support/docker_postgres.rs` (ephemeral
  `postgres:16`, random port, force-removed on `Drop`, **skips cleanly with no daemon** so
  `cargo test --all-features` stays green infra-free). It loads the commerce schema lowered for
  `Dialect::Postgres` explicitly (the manifest is `mariadb`, so it re-lowers via `Compiled::from_checked`,
  since Postgres genuinely differs — `$n` binds, `"`-quoting, native `uuid`/`jsonb`), creates tables from
  the *generated* Postgres DDL, and drives the **verbatim** codegen-lowered SQL through `serve::dispatch`
  against a `PostgresDb` checked out of a live `PgRouter`: get/list, `$ctx` row-scope + joined-`ON` reach,
  write + declared-shape re-select under one tx (read-your-writes, proving the engine-generated uuid
  round-trips the `uuid` column), idempotency dedupe, `Backend::ping`. **Ran genuinely green against real
  Postgres 16** (not compile-verified) — this is now the `PostgresDb` driver's gate, and `docker-tests`
  enables the `postgres` feature so the suite runs alongside the MariaDB one.
- *Deferred (Track A4, same as MariaDB):* typed JSON reconstruction (a `jsonb` column comes back as a
  JSON-encoded *string*, not a reconstructed object — the runtime carries no per-column types into row
  shaping), statement timeouts, deadlock-retry, pool-exhaustion → 503 under load — designed (D20), not
  yet stress-proven live.

## D39 — migration snapshot + diff engine (Track E2)
The snapshot + diff half of migration generation (`based-codegen::migrate`; CLI `based migrate gen`).
D37 pinned the *artifact layout* + `@was`; migrations.md (E1) wrote the *model*; this records the
*implementation* decisions E2 had to commit to (the spec left the snapshot serialization + step-data
shape as E2's TODOs).
- **Placement: `based-codegen::migrate`, not a new crate.** The snapshot/diff format is decoupled from
  SQL text (it names no dialect), but E3's per-dialect renderer lowers the neutral `Step`s over the
  *same* `Dialect` seam the DDL/DML emitters already use, so the migration engine lives beside them —
  one crate owns "neutral IR → target" (P4, one lowering seam). No `based-migrate` crate.
- **`schema.snap` grammar (finalizing migrations.md's TODO).** A stable-ordered indented text block in
  the schema's own neutral vocabulary — *not* JSON, *not* SQL (dialect-neutral: `int`/`text`/`uuid`,
  never `BIGINT`; a `default=now()`, never `CURRENT_TIMESTAMP`):
  ```
  snapshot v1 dialect=neutral
  table <name> [soft_delete=<col>:<mode>] [created=<col>] [updated=<col>] [scope=(<col> = $ctx.<f>, …)] [sort=(<col> <dir>, …)]
    column <name> <type> null|not_null [default=<lit>] [unique] [fk=<Model>]
    index  <name> (<col>, …) [unique] [inferred]
  ```
  Determinism is the hard invariant (the loop forbids wall-clock in reproducible library paths): tables
  sorted by name, columns + indexes sorted by name within a table, every derived name (`inf_`/`idx_`/
  `uq_`, FK `<field>_id`) reproduced exactly as `sql::ddl` names it. A relation is its FK column
  (`fk=<Model>` records the target so a retyped/dropped relation diffs as an add/drop/alter of
  `<field>_id`, D3). The default `id` (uuid/not-null/not-unique, D2) is **elided** and carried as an
  invariant; a non-default `id` records itself. Soft-delete/`@created`/`@updated` roles + `@scope`/
  `@sort` ride the table header (they emit no column step but must round-trip so drift stays honest).
  `schema.snap` is **timestamp-free** — a migration timestamp, if ever wanted, is CLI-layer metadata,
  never snapshot content (keeps `schema.snap` byte-stable across runs).
- **Round-trippable.** `Snapshot::render`/`Snapshot::parse` are inverses, so the stored baseline parses
  back to the identical neutral model a `diff` compares against — the property the offline diff (no DB)
  relies on. A corrupt/hand-edited `schema.snap` is a loud `ParseError(line, message)`, never a silent
  mis-diff.
- **Neutral step vocabulary + destructive *marking* (not gating).** `diff(prev_snapshot, schema)` →
  `Vec<Step>` in migrations.md's `up.mig` vocabulary (create/drop table, add/drop/alter column, add/drop
  index/unique). `0001_init` (empty prior) is a full create set == `based gen sql` from scratch. Renames
  are **drop + add**, never auto-guessed (the `@was` RENAME step is E5). E2 *marks* a data-losing step
  `Step::destructive()` — drop table/column, a **narrowing** type change (anything but a widen to `text`,
  conservative per P1), a new `not_null` without a default, a new `unique` over existing data — so E4's
  apply can gate on `--allow-destructive`/`unsafe("reason")`; E2 marks, **never applies** (offline).
- **`based migrate gen [name]`** loads the checked schema (reuses `load_checked`), reads the highest-
  `NNNN` `migrations/*/schema.snap` (empty if none), diffs, and — only if the step list is non-empty —
  writes the next zero-padded `NNNN_slug/{up.mig, schema.snap}`. **`NNNN` comes from counting existing
  dirs, not time** (determinism). Slug = the snake-cased `[name]` arg, else `init` (first) / `schema_
  update`. No changes ⇒ writes nothing, exits clean. Fully offline — no database.
- *Deferred to E3/E4/E5, unchanged:* the per-dialect SQL render of the neutral steps + the `raw(dialect)`
  passthrough + `based migrate render` (E3); apply + the `_based_migrations` ledger + hash/tamper rule +
  the destructive **gate** + `verify`/`status` (E4); the `@was` RENAME step + the offline LSP drift
  diagnostic (E5). The raw-step structural-effect annotation (migrations.md's other open TODO) is
  untouched — E2 emits no `raw` steps.

## D40 — LSP per-file manifest resolution (embedded schemas; Track C3)
The editor server must find a `.bsl` file's project *the way rust-analyzer/tsserver do* — by the nearest
project marker above it — not by assuming the opened workspace folder is the schema root. The designed
layout (D9) has `.bsl` riding along **inside a host repo**, so the opened folder is almost never the
schema's `based.toml` dir; the old "root at one folder, `discover(root)`, else overlays-only" model
compiled each buffer in isolation and reported spurious `E0110 unknown model` on valid cross-file refs
(the language has no imports — the manifest glob *is* the namespace, D5/D9).
- **Project marker = the nearest ancestor `based.toml`.** `compile::find_manifest_root(file)` canonicalizes
  the file, then walks parent dirs until one holds a `based.toml`; that dir is the project root. `None` ⇒
  the file rides under no project. An unsaved buffer with no on-disk path falls back to its raw path's
  ancestors (still meaningful).
- **One snapshot per project, keyed by owner.** `ProjectKey` = `Manifest(root_dir)` | `Loose(file)`. Each
  open buffer resolves to exactly one key; `refresh` compiles a snapshot per **distinct** key —
  `compile_manifest(root)` runs the D5 `**/*.bsl` glob (so sibling models resolve), `compile_loose(file)`
  keeps the single-file fallback for a file under no manifest. Multiple embedded schemas in one workspace
  are therefore compiled **independently**; a request (`hover`/`inlay`/diagnostics) routes to
  `snapshots.get(&project_key(path))`. `State` dropped the single `root`/`snapshot`.
- **A file is published from its owning project only.** A nested manifest's file also matches an outer
  project's glob, so on publish each file is emitted from the snapshot whose key equals *its* nearest
  manifest — the nearest owns it, no double-publish. A `published: Vec<Url>` records last-published files so
  a project dropping out of the open set (its last buffer closed) has its squiggles explicitly cleared.
- **Lazy, per-file.** No proactive whole-workspace scan on `initialize`; projects compile as their files
  open (an editor surfaces diagnostics per open file anyway). *Deferred:* a brand-new unsaved file isn't in
  its manifest's on-disk glob, so it compiles loose until first save (pre-existing edge).

## D41 — per-dialect migration renderer (`migrate::render_sql`, E3)
E2 (D39) produces the dialect-neutral `up.mig` step list; E3 renders it to executable SQL per dialect. How
(`based-codegen::migrate::render_sql`; CLI `based migrate render`), settling migrations.md's E3 details:
- **One type map, no drift.** The renderer maps neutral snapshot types (`int`/`text`/`uuid`/…) through the
  *same* `sql::sql_type` the DDL uses (now `pub(crate)`) and quotes via `Dialect::quote`, so a migration's
  SQL cannot diverge from `based gen sql` (P4). `0001_init`'s `create table` steps render to exactly the
  from-scratch DDL (verified: one `CREATE TABLE` + `PRIMARY KEY` per model per dialect). `CreateTable`
  re-synthesizes the implicit `id` PK the snapshot elides (D2); `(unique)` columns → `CONSTRAINT … UNIQUE`;
  indexes are inline `KEY`/`UNIQUE KEY` on MariaDB, trailing `CREATE INDEX` on SQLite/Postgres (mirroring
  `sql::create_table`). Column `NULL`/`NOT NULL` is stated explicitly in all three (valid on each — SQLite
  accepts a bare `NULL` constraint, verified against a real server; the migrations.md examples were updated
  to match).
- **`alter column` diverges by dialect — the one place the neutral vocabulary can't be uniform.** Postgres
  has piecemeal `ALTER COLUMN … TYPE/SET NOT NULL/DROP NOT NULL/SET DEFAULT/DROP DEFAULT`, one sub-statement
  per change. MariaDB/MySQL have no piecemeal null/type change — a structural change needs a full
  `MODIFY COLUMN <whole definition>`, so `Step::AlterColumn` grew an `after: ColumnSnap` (the resulting
  column) for the renderer to restate; a default-only change still uses `ALTER COLUMN … SET/DROP DEFAULT`
  (no MODIFY). **SQLite has *no* in-place `ALTER COLUMN`** (a type/null/default change needs the 12-step
  table rebuild), which the neutral vocabulary can't safely auto-generate — so it renders a loud, greppable
  comment pointing at a hand-authored `raw(sqlite)` step rather than broken SQL (principle 6 — the escape
  hatch is never silent). `DROP INDEX` also branches (MySQL/MariaDB require `ON <table>`). Destructive steps
  carry a loud `-- DESTRUCTIVE` marker (principle 1).
- **`render` re-derives steps from the stored snapshots, not an `up.mig` parser.** `based migrate render
  [--number NNNN] [--dialect D]` computes migration N's steps as `diff_snapshots(snapshot[N-1], snapshot[N])`
  from the stored `schema.snap`s — the snapshot-authoritative model migrations.md defines, and exactly what
  `verify` asserts equals the `up.mig`. So no `up.mig` text parser is needed for render; that parser lands
  with **E4 (apply)**, which must parse *and* content-hash `up.mig` for the ledger. Consequence (documented,
  deferred): render reflects the canonical snapshot delta, so a *hand-edited* `up.mig` isn't honored until
  the E4 parser exists — fine for generated migrations, which are the norm. `--dialect` overrides the
  manifest target for a cross-target review; the default is the manifest dialect. Render is fully offline
  (reads stored artifacts, never a DB) and does not run the front end, so it works against an in-progress
  schema.
- **Proven executable, not compile-verified.** The commerce `0001_init` + an incremental `0002` (add nullable
  column + a nullable-alter + an index) render was applied end to end against real `sqlite3`, `postgres:16`,
  and `mariadb:11.4` (Docker/OrbStack): every dialect's create, add-column, alter-column, and index SQL
  runs cleanly (the `MODIFY COLUMN`/`ALTER COLUMN` paths flip `name` to nullable; SQLite skips the alter with
  its comment). *Deferred:* the `raw(dialect)` passthrough step (the `Step` enum has no raw variant yet —
  migrations.md's raw-structural-effect TODO); rename steps (E5, `@was`); the `up.mig` parser for hand-edits
  (E4).

## D42 — migration apply + `_based_migrations` ledger (E4)
E4 carries a real database from one migration state to the next. Engine: `based-runtime::migrate`
(`load_migrations` / `ensure_ledger` / `applied` / `apply` / `status`, dialect-generic over the `Db` seam),
plus `based-codegen::migrate::{sql_statements, content_hash}` (the offline halves) and the CLI's
`based migrate apply|status|verify`. Settling migrations.md's E4 TODOs:
- **Execution is snapshot-authoritative, not up.mig-parsed.** A migration's executable steps are re-derived
  as `diff_snapshots(snapshot[N-1], snapshot[N])` (the same model `render` uses, D41) and lowered by the new
  `migrate::sql_statements` — the execution twin of `render_sql`, returning **bare** statements (no `;`, no
  comments) so `Db::execute` runs them one at a time. Both go through one `step_statements` seam, so the SQL
  *applied* is exactly the SQL *reviewed* (P4). This resolves D41's deferred "up.mig parser": there is none —
  the neutral `up.mig`/`down.mig` text is **not** losslessly parseable back to `Step`s (render_up drops each
  index step's table and each `alter column`'s resulting `after` state), and the snapshot chain is the honest
  source of truth. Consequence (documented): a *hand-edited* `up.mig` still isn't honored on the up path — its
  steps come from the snapshots; the edit is caught by `verify` (below) and the tamper hash.
- **The `_based_migrations` ledger** (id text PK + content_hash + applied_at, dialect-typed) is created on
  first use (`ensure_ledger`, `CREATE TABLE IF NOT EXISTS`). `apply` runs each migration's statements **plus
  its ledger insert under one `begin`/`commit`** (principle 7); a failed statement rolls back. On MySQL/MariaDB
  DDL implicitly commits, so the tx is best-effort there — the ledger row is still written in the same turn and
  a re-`apply` skips completed migrations (matched by id), so a crash mid-apply retries cleanly.
- **`content_hash` = FNV-1a-64 over the comment/blank-stripped, line-trimmed `up.mig` bytes**, 16 lowercase
  hex (the D31 fingerprint family; not security-critical — it guards an accidental post-apply edit). Recorded
  in the ledger; at every `apply`/`status` an applied migration's *current* up.mig hash is compared to the
  stored one — a **mismatch is a hard error** (`MigrateError::Tamper`), never a silent re-apply (migrations.md:
  applied history is immutable, fix forward). An applied row whose directory is gone, or a non-prefix/gapped
  ledger, is likewise a loud `Order` error.
- **Destructive gate.** A pending migration with any destructive step (drop / narrowing / new
  not-null-without-default / new unique — `Step::destructive`, D39) refuses to apply without
  `--allow-destructive`; the safe migrations before it still apply, then `apply` stops at the gate (principle 1).
- **Rollback = raw-SQL `down.mig`, honored if present, never generated.** A `down.mig` is **raw per-dialect
  SQL** (`;`-split), not neutral steps — because the neutral text isn't losslessly parseable (above) and a
  hand-written reverse is naturally SQL (mirrors the `raw(dialect)` escape). `Direction::Down` rolls back the
  latest applied; `Direction::To(N)` reconciles the applied set to `{≤ N}` (roll forward pending up to N, or
  roll back — newest first — anything above it; `To(0)` = all), each rollback deleting its ledger row in the
  same tx. A rollback with no `down.mig` is `MigrateError::NoDown` (roll-forward only), never a silent skip.
- **`based migrate verify` = the offline CI gate.** For each migration it re-renders `render_up(diff(snap[N-1],
  snap[N]))` and compares its `content_hash` to the stored `up.mig` (catching an up.mig hand-edited away from
  its snapshots), checks the numbering is gap-free, and asserts the *latest* snapshot equals
  `Snapshot::from_schema(current .bsl)` (catching uncaptured schema changes — the CLI twin of the offline LSP
  drift diagnostic, E5). No database. Reads clean today; `raw`-carrying migrations would report `partial` once
  raw steps land.
- **Multi-dialect + multi-shard.** `apply`/`status` connect through the same driver stack `based serve` uses —
  the CLI now links all three drivers (`ShardRouter`/`PgRouter`/`SqliteBackend`) and picks by manifest dialect;
  `--database-url` is repeatable so a sharded fleet migrates every shard with the same set (D20). Proven live:
  `tests/migrate_apply.rs` (SQLite in-memory, in the normal gate — fresh apply + ledger, re-apply no-op,
  `status`, `down.mig` rollback, tamper, destructive gate) and `tests/migrate_apply_mariadb.rs` (the D35 Docker
  harness — apply against a real `mariadb:11.4`, ledger + column verified, re-apply no-op, tamper), both ran
  genuinely green. *Deferred:* multi-instance apply coordination (advisory lock for racing deployers, parallels
  D25's durable-store deferral); `@was` renames + the LSP drift diagnostic (E5); the `raw(dialect)` up step.

## D43 — LSP go-to-definition + type-name syntax coloring (Track C follow-up)
The first non-diagnostic IDE ergonomic, plus the coloring that makes types legible. Both are ordinary
product calls (principle 8 neither requires nor forbids them), sequenced after the VS Code client (D36).
- **Go-to-definition = reference-collection over the retained AST, no new resolver.** `Snapshot` now keeps the
  parsed `decls` (the same AST sema checked); `Snapshot::definition_at(fid, offset)` walks *every* AST position
  where a name *points at* a declared type — field types (`BaseType::Model`), opt-in inverses (`InverseRef.model`),
  shape `from`, query/mutation return types (`RetType.ty`) + param types (`Param.ty`) + `get`/`list` targets
  (`Statement.model`), write targets (`WriteStmt::{Create,Update,Delete,Restore,HardDelete}.model`, recursing
  through `tx`), and filter param types — finds the reference `Ident` whose span covers the cursor, and resolves
  it to the `Model` **or** `Shape` declaration of that name, returning that decl's *name* span. (Shapes included
  because a return type may name a shape, not just a model — strictly more useful at no extra cost.) Cross-file:
  routed to the snapshot *owning* the requested file (nearest `based.toml`, D40) exactly as hover/inlay, so a
  reference resolves to its declaration in a sibling file. `None` when the cursor is off any reference or the
  type is undeclared — the definition is *not* invented, matching the diagnostic that already flags it. This is
  a single-target lookup, deliberately **not** the full reference-site index rename needs (still deferred): the
  collector finds one match under the cursor rather than indexing all uses of a symbol.
- **Type-name coloring = a PascalCase heuristic, not semantic resolution.** TextMate is regex-based, so the
  grammar (`editors/vscode/syntaxes/bsl.tmLanguage.json` `#types`, after `#keywords` so lowercase keywords win)
  has two rules, primitives *first*: builtin scalars (`text|int|bool|timestamp|date|json|uuid|Id` — the exact
  `Primitive` variants) → `support.type.primitive`, then any PascalCase word (`[A-Z]…`, D7's model-name
  convention) → `entity.name.type`. Model refs get a distinct theme color from builtin scalars and from
  lowercase field names/keywords. Precision is intentionally the heuristic's, not the semantic analyzer's — a
  PascalCase word *is* a type reference by convention; the LSP already carries the exact diagnostics.

## D44 — LSP document symbols + the Track C4 capability audit (Track C4)
The extension's feature-parity fill-in (C4) opens with the explicit capability checklist (in
`editors/vscode/README.md` — each standard LSP capability marked have / missing / N/A / deferred, so the gap
set governing the remaining C4 iterations is reviewable) and the highest value-per-effort gap it names:
document symbols.
- **Document symbols = a flat pass over the retained AST, same source as go-to-def (D43).** `Snapshot::
  document_symbols(fid)` walks the parsed `decls`, emits a `DocumentSymbol` for every decl *declared in the
  requested file* (`span.file == fid` — a project snapshot spans many files, but the outline is per-file), and
  anchors each to two spans LSP requires: `range` = the decl's whole extent, `selection_range` = its name (the
  latter contained in the former, so the tree nests). Routed to the snapshot owning the file (nearest manifest,
  D40) like every other position request.
- **Symbol-kind mapping** (the resolvable design call): model → `STRUCT`, its fields → `FIELD` **children**
  (nested under the model; indexes / soft-overrides are not symbols), shape → `INTERFACE`, query → `FUNCTION`,
  mutation → `METHOD`, filter → `FUNCTION`. Chosen so the outline reads like the schema's own vocabulary — a
  model is a record (Struct) of fields, a shape is a projection contract (Interface), a query reads (Function)
  and a mutation writes (Method). Only fields nest; everything else is a flat top-level symbol (queries/
  mutations/shapes/filters own no sub-declarations).
- **Client-side: nothing to wire.** `document_symbol_provider` is advertised at `initialize`; `vscode-
  languageclient` negotiates the outline automatically (verified — same as inlay/hover/definition). The
  reference-site index that find-references + rename will reuse is still the deferred go-to-def resume point;
  document symbols does not build it (it finds decl *sites*, not use *sites*).

## D45 — LSP completion (Track C4)
The gap the C4 audit calls the one an author feels constantly, and the next C4 fill-in after document symbols
(D44). `Snapshot::completions(fid, offset)` over the retained `decls`, advertised via `completion_provider`
(trigger chars `.` and `@`); `vscode-languageclient` negotiates it with no client change.
- **Context by source prefix, not by parsing the mid-edit buffer.** The buffer under an edit is routinely
  unparseable (a half-typed `order.`), so the resolver never re-parses it — it reads the source *prefix* before
  the cursor, strips the partial word + trailing spaces, and dispatches on the exposed **trigger character**
  (the token immediately before the word being typed). This is the pragmatic heuristic the task called for; it
  is O(prefix) and independent of parse state, so completion works even while the file has errors.
- **The five contexts** (kinds in parens): `@` → the decorator set (`PROPERTY`); `<ident>.` → the base model's
  fields (`FIELD`); `:` → primitives + model names (a field's type annotation — `KEYWORD` + `STRUCT`); `->` →
  models + shapes (a return type — `STRUCT` + `INTERFACE`); anything else → the keyword + function vocabulary +
  model names (`KEYWORD`/`FUNCTION`/`STRUCT`).
- **Field completion is precision-over-recall (matches W0103's stance).** Fields are offered *only* when the
  dotted path's base is statically resolvable: the enclosing decl must give a cheap root model — a shape's
  `from` or a query block's target (`root_model_at`) — and each path segment before the `.` must be a relation
  field, walked to its target model (`field_items_after_dot`). Any non-relation segment, unknown root, or a
  context without a cheap root (a mutation write body — no `WriteStmt` span; an inline/bare query — target only
  in the IR, not the AST; `^.`/`$ctx.`) returns nothing rather than wrong suggestions. So `org.` inside a shape
  completes the related model's fields; `total.` (a scalar) and `^.` complete nothing.
- **The exposed sets are derived, not invented.** Keywords = the parser's positionally-recognized keyword
  strings (grep `eat_kw`/`at_kw`) — note there is **no `model` keyword** (a model is a bare `UpperName { … }`),
  so it is deliberately absent. Decorators = `based_sema::KNOWN_DECORATORS` (`soft_delete`/`sort`/`scope`/
  `created`/`updated`/`table`) + the member-level `@index` + the `@was("old")` rename directive (E1 grammar;
  not yet in `KNOWN_DECORATORS` since E5's sema is pending, but a real author-typed decorator). Primitives =
  the `Primitive` variant spellings (`text`/`int`/`bool`/`timestamp`/`date`/`json`/`uuid`/`Id`). Functions =
  `based_sema::KNOWN_FUNCS`. Sourcing from the sema consts keeps the sets from drifting.
- **No fuzzy ranking / snippets** — each item is a bare label + kind; the client filters by the typed prefix.
  The reference-site index that find-references + rename will reuse is still the deferred go-to-def resume
  point (`collect_type_refs` finds one use under the cursor, not all uses of a symbol).

## D46 — named scope: a `scope` decl referenced by `@scope Name` / `scoped Name` (spec-only)
Promotes `@scope` from an inline, inferred, editor-hint-only predicate (D32) to a **first-class named
declaration referenced on both sides** — the model it governs and every callable that touches it. The
user's framing (2026-07-07): "declare it like a shape, reference it briefly on both sides; a scope
contract this important must be **written, not implied**." The old form was implied (the `$ctx` type
was *inferred* per callable and only *shown* as an editor hint, D4/D5) — principle 2 forbids that for a
consequential contract. This is **spec-only**; parser/sema/codegen follow in later iterations (see PLAN).
- **`scope Name (col: Type = $ctx.field, …)` — the decl** (grammar `scope_decl`, a new top-level `decl`).
  The predicate keeps the D32 restricted form (a conjunction of `col = $ctx.field` equalities, `E0180`) —
  that restriction is still what makes a scope injectable everywhere and auto-settable on `create`. What
  is new: the column's type is **declared here** (`org: Org`), and by the equality it is *also* the
  `$ctx.field` type. So the scope decl is the one source of truth (P4) for the scope field's type.
- **`@scope Name` on a model** (grammar `scope_deco`, a distinct decorator form — bare name, no
  parenthesized predicate, mirroring `@index barcode`). The predicate is no longer restated on the model
  (P4). A governed model must declare the scope's column at a conforming type (`E0184`); a physical-name
  divergence is aliased at the field (`(column "…")`, D3/D8). A per-model *field-name* override
  (`@scope Tenant(owner_org)`) is **reserved but deferred** — v1 requires the field name to equal the
  scope column name (minimal; the common case is a uniform column across models, which is the point).
- **`scoped Name[, Name]*` on a callable** (grammar `scoped_clause`, sits where `unscoped_clause` sits —
  after the return type / after any `guard`; mirrors `guard name`'s bare-name form). The
  **required-declaration rule:** a callable whose target is scoped MUST write *either* `scoped …` *or*
  `unscoped("reason")` — omitting **both** is a hard `E0182` (written, not implied). Multi-scope: a query
  reaching a second scoped model via a relation (D34 joined-`ON`) names both, comma-separated; the
  declared set must exactly match the callable's actual scope set (root + joined reaches) or `E0185`. This
  makes D34's joined-scope enforcement *visible in source* — the reader sees every boundary crossed.
- **`unscoped("reason")` unchanged** (D32): the cross-scope escape hatch, mutually exclusive with
  `scoped`, mandatory reason, greppable, `W0106` when stale (target in no scope).
- **Coherence shift:** because the scope field's type is declared once in the `scope` decl, the old
  cross-callable `$ctx` coherence check for the scope field (`E0161`) becomes **structural** — one decl,
  one type, no clash possible. `E0161` still guards non-scope `$ctx` fields (Handle-1 `where`s, guard
  args) whose types are still inferred per callable (D4). This removes the inference that leaked
  `(D4/D5)` into hover.
- **Error set (E018x band, continuing D32's E0180/E0181):** `E0182` missing `scoped`/`unscoped`
  acknowledgement on a scoped callable; `E0183` unknown scope name (`@scope`/`scoped` names no `scope`
  decl); `E0184` scope column missing/non-conforming on a `@scope` model; `E0185` a callable's `scoped`
  set ≠ its actual scope set. `E0180` (predicate form) now fires at the `scope` decl site; `E0181` (create
  assigns scope col) and `W0106` (stale unscoped) unchanged. A duplicate `scope` name reuses the general
  duplicate-decl mechanism (like a duplicate shape).
- **Migration-snapshot note (E-track follow-up, do NOT drift E2/D39).** `schema.snap` today records each
  table's scope inline in the header (`scope=(org_id = $ctx.org)`, D39). Under named scopes the snapshot
  must instead: (a) serialize the `scope` **decls** as top-level `scope <Name> (<col>:<type> = $ctx.<f>, …)`
  lines (stable-ordered, before the tables), and (b) record each table's scope **by name**
  (`scope=<Name>`). Same DDL (a scope emits no column), so this is header/metadata only — but it must
  round-trip so an offline diff can detect a scope rename, an added term, or a model joining/leaving a
  scope. **Flagged as snapshot-format work for the E-track** (the serializer change lands with the sema
  implementation, not this spec iteration).
- **Facts/LSP acceptance note (impl iteration).** The scope contract must be surfaced as a *written,
  referenceable* thing: go-to-def from `@scope Name` / `scoped Name` to the `scope` decl, rename across
  all refs, hover naming the scope. **No decision-record references (D-numbers) may appear in any
  editor-facing string** — the user reported `(D4/D5)` leaking into hover; this design removes the
  inference that produced it, and the impl must not reintroduce D-numbers in hints/hover.
- **Open sub-questions for the user (review checkpoint):** (1) the term form `col: Type = $ctx.field` —
  type on the column side (chosen: it doubles as the governed-model column contract) vs. annotating the
  ctx field (`col = $ctx.field: Type`); (2) whether the per-model column override (`@scope Name(field)`)
  should ship in v1 or stay reserved (chosen: reserved/deferred).

## D47 — multi-scope: `@scope` repeatable; AND (one decorator) / OR (stacked) alternatives (DNF)
Generalizes D46's single scope-per-model to a **set** of scopes, settled with the user 2026-07-07. The
`scope` **decl** is unchanged (D46 — `scope Name (col: Type = $ctx.field, …)`, predicate a conjunction of
`col = $ctx.field`, the one place a scope field's `$ctx` type is declared); only the *model reference*
and the *callable/create semantics* generalize. Revises D46.
- **`@scope` is repeatable; the stack is a disjunction of conjunctions (DNF).** Commas *within* one
  `@scope` decorator are an AND-conjunction — **one alternative**, all named axes required together.
  Stacked `@scope` decorators are OR-alternatives. `@scope Page, Author` = one alternative `{Page ∧
  Author}`; `@scope Page` + `@scope Author` (two lines) = two alternatives `{Page}`, `{Author}`; mixing
  gives `(Page ∧ Author) ∨ Admin`. No new syntax — comma is AND, new line is OR (grammar `scope_deco`
  now `'@scope' scope_name { ',' scope_name }`, repeatable via the existing `{ model_deco }`).
- **Syntax rationale.** Each `@scope` decorator declares *one valid way to be scoped* (one confinement).
  Stacking = "any of these confinements is acceptable"; commas = "this confinement needs all of these
  axes." Reads top-to-bottom as an enumeration of the sanctioned confinements — a static, greppable
  contract, never a runtime disjunction.
- **The uniform callable rule.** A callable must confine by a set of scope axes that is a **superset of
  at least one** declared `@scope` alternative of *each* scoped model it touches (root + D34 joined
  reaches) — else `unscoped("reason")`. AND model → `scoped` must name both axes; OR model → either
  alternative's axes suffice. Naming extra/narrower axes is safe (more confinement never leaks); naming
  an axis no touched model declares, or too few to satisfy any alternative, is `E0185` (revised from
  D46's "set ≠ actual set" to "⊇ one alternative"). Reads inject the **conjunction of the named axes**
  into every `WHERE` + joined `ON` (the chosen alternative). This vindicates the user's earlier
  "input ⊇ allowed scopes" intuition, made precise to confinement axes.
- **Create safety (`E0186`, new).** Scope columns stay engine-managed (`E0181` — a create can't assign
  them). A create auto-sets every scope column whose `$ctx` field is available and **must satisfy ≥1 of
  the target model's alternatives** (all axes of some `@scope` set), so no row is created unowned —
  closing the accidentally-unfiltered hole on the *write* side. A create that can satisfy no alternative,
  or whose required non-null scope column has no `$ctx` value, is `E0186`.
- **Error set (revises D46).** `E0182`/`E0183`/`E0180`/`E0181`/`W0106` unchanged; `E0184` unchanged, now
  checked per `@scope` decorator. `E0185` **revised** to the superset-of-an-alternative rule. `E0186`
  **new** (create satisfies no alternative / required non-null scope col absent).
- **Guard boundary (unchanged, sharpened).** Multiple `@scope` alternatives are still *static*
  confinement — a callable picks **one** at author time, and the returned row set differs by which
  (`posts_on_page` ≠ `my_posts`). A *runtime* `WHERE a OR b` where the returned data is identical and you
  only check a credential stays Handle 3 (`guard`), NOT a scope. Rule: disjunction that changes *which
  rows* → `@scope` alternatives; disjunction that only gates *whether the same rows* → `guard`.

## D48 — named scope landed: `scope`/`@scope Name`/`scoped Name` replaces inline `@scope(pred)`
Implements D46 (Track G, iteration 1): the inline, per-callable-inferred `@scope(pred)` (D32) is gone;
scope is now a first-class **named** decl referenced by name on both sides. Parser/AST/sema all ship;
codegen + runtime are unchanged in *effect*. Multi-scope DNF (D47) is the next iteration.
- **AST/parser.** New `Decl::Scope(ScopeDecl)` (`scope Name (col: Type = $ctx.field, …)`, each term a
  `ScopeTerm { col, ty, ctx }`). `@scope Name[, Name]*` parses into `Model.scopes: Vec<ScopeRef>` (a
  distinct decorator form, special-cased in the model-decorator loop — bare names, never the generic
  parenthesized `Decorator`). `scoped Name[, Name]*` parses into `Query.scoped`/`Mutation.scoped:
  Option<Scoped>` via a shared `scope_ack` (mutually exclusive with the unchanged `unscoped`). `scope`
  is a new top-level decl keyword; `scope`/`scoped` are contextual (D8).
- **Sema (`scope.rs`).** `resolve_decls` builds `CheckedSchema.scopes: Vec<RScope>` (`RScopeTerm { column,
  ctx_field, ty: CtxField }`) — the `col: Type` is where the scope field's type is declared, checking the
  binding is `$ctx.<field>` (`E0180` now fires at the decl, not per callable) and the term type resolves
  (dup name → `E0105`). `attach_models` resolves each model's `@scope` refs → `RModel.scope_alts:
  Vec<Vec<String>>` (a DNF list, forward-compat for D47 though iteration 1 uses the single alternative),
  checks each named scope's column exists on the model at a conforming type (`E0184`), reports unknown
  names (`E0183`), and **synthesizes `RModel.scope: Option<Predicate>`** = the AND of the chosen
  alternative's `col = $ctx.field` terms. Keeping `RModel.scope` as a synthesized predicate is the key
  impl decision: `scope_terms()`, `shard_key_ctx_field()`, and **all of codegen/runtime are untouched** —
  they lower the same predicate the old inline form produced (a good regression check; the codegen golden
  SQL is unchanged, the sema conformance golden is byte-identical). Callable acknowledgement is checked
  against the scoped models a callable *touches* (root + D34 joined reaches, walked in `scope.rs` the same
  way `ctx.rs` walks joins): neither `scoped` nor `unscoped` on a scoped target → `E0182`; `scoped` naming
  an unknown scope → `E0183`, or a scope no touched model declares / too few axes for any alternative →
  `E0185`. `W0106` (stale unscoped) now keys on the touched set being empty.
- **`$ctx` type sourcing (ends D4/D5 for the scope field).** Because the scope column conforms to the
  decl type (`E0184`), inferring the scope field's `$ctx` type from the synthesized predicate's column
  yields exactly the declared type — so coherence for the scope field is structurally consistent (never a
  clash); non-scope `$ctx` fields are still inferred per callable (`E0161` unchanged). `CheckedSchema`
  exposes `scope(name) -> Option<&RScope>` for facts/LSP (iteration 3).
- **Pass order.** Scope resolution slots between model `validate` and the `Cx`-based check pass (`Cx`
  gained `scopes`/`scope_index`); `resolve.rs::check_scope_form` and model.rs's `@scope`-decorator arms
  were removed (dead — `@scope` no longer lands in `Model.decorators`).
- **Commerce migrated end-to-end.** `spec/examples/commerce/order` now declares
  `scope Tenant (org: Org = $ctx.org)`, puts `@scope Tenant` on `Order`, and marks the three Order
  queries + `place_order` `scoped Tenant` (the cross-org `orders_in_org` stays `unscoped`). `based check
  spec/examples/commerce` is clean.
- **E0186 deferred** to iteration 2 (a single-alternative create with a present-by-auto-set scope column
  can't be unsatisfiable, so it isn't reachable in the single-scope world). The `SCOPE_CREATE_UNSAT`
  code is registered for it.
- **Left ready for iteration 2 (multi-scope DNF, D47).** `Model.scopes`/`RModel.scope_alts` are already
  lists of alternatives; `scoped`/`@scope` already parse comma-separated name sets; `check_ack` already
  implements the superset-of-an-alternative rule over a `Vec` of alternatives. Remaining: make codegen
  inject the *callable-chosen* alternative's conjunction (today it synthesizes the single alternative's
  predicate onto `RModel.scope`); `E0186` (create satisfies ≥1 alternative); the migration `schema.snap`
  serializer (record scopes by name + a model's alternative set — the E-track snapshot change flagged in
  D46/G5); facts/LSP go-to-def/rename/hover over scope refs.

## D49 — multi-scope DNF made real: per-callable alternative injection + E0186
Implements D47 (Track G, iteration 2): a model's stacked `@scope` decorators are now a live DNF —
codegen injects the *alternative the callable chose*, not the one synthesized predicate D48 parked on
`RModel.scope`. Codegen/runtime output for a single-alternative model is **byte-identical** to D48 (the
commerce/codegen goldens are unchanged — the regression proof); only multi-alternative schemas differ.
- **Per-callable resolved injection (sema → codegen).** Sema resolves, per callable, a
  `Vec<ScopeInject>` (`{ model, terms: Vec<(column, ctx_field)> }`) — one entry per *touched* scoped
  model (root + every D34 joined reach), threaded onto `RQuery`/`RMutation.scope_inject`.
  `scope::resolve_inject` computes each entry's terms as the callable's named axes (`scoped …`) that the
  model carries, expanded through the `scope` decls, deduped in decl order. `E0185`'s superset rule
  guarantees the terms include ≥1 whole alternative, so a model is never left unfiltered; naming extra
  axes only narrows (never leaks). `unscoped` → empty (no injection).
- **Codegen consumes the map, not `RModel.scope`.** `Select` carries the callable's `&[ScopeInject]`
  (`with_scope_terms`); a new `Select::scope_where(alias, model)` builds `<alias>.<col> = :ctx_<field>`
  ANDed from the chosen terms, replacing every prior read of `RModel.scope`/`scope_terms()` in the SQL
  path — root `WHERE` (`dml`), joined `ON` (`scope_join_pred`), the create auto-set, the write-`WHERE`
  guards (`inject_guards`), restore, and the D12 re-select. For a single-axis single-alternative model
  the emitted string is identical to the old `sel.predicate(&root.scope, …)` (same `physical_col` + the
  same `:ctx_<field>` bind), so nothing in `based gen sql` moves. `RModel.scope`/`scope_terms()` stay —
  still the source for `shard_key_ctx_field` (D33) and the create-time `E0181` guard.
- **`E0186` (create satisfies an alternative), `scope::check_create_sat`.** A `create M` on a scoped
  model must name a **full** `@scope` alternative of `M` so the engine can auto-set all its columns from
  `$ctx` (no half-owned row). Trigger: the mutation's `scoped …` set is a superset of *no* alternative of
  `M` — e.g. an AND model `@scope Page, Author` whose create names only `Page`, leaving `Author`'s column
  with no `$ctx` value. Fires at the create's model span; skipped for `unscoped` (auto-set dropped, caller
  owns the columns). It co-fires with `E0185` on a scoped-but-insufficient mutation (auth.md endorses
  this — the AND worked example shows both), and is clean when a whole alternative is named. Registered
  code `SCOPE_CREATE_UNSAT` is now wired.
- **Fixtures.** A worked OR + AND pair proves it: an OR model (`@scope Page` + `@scope Author`) where
  `posts_on_page scoped Page` injects `` `post`.`page_id` = :ctx_page `` and `my_posts scoped Author`
  injects `` `post`.`author_id` = :ctx_user `` — the *same model*, a *different* predicate per callable;
  an AND model (`@scope Page, Author`) injects both axes ANDed, and its create auto-sets both scope
  columns; `scoped Page` alone on the AND model is `E0185`, and an AND-model create naming one axis is
  `E0186`. (Codegen `tests/dml.rs`/`tests/mutations.rs` + sema `tests/check.rs`.)
- **Deferred to iteration 3 (unchanged from D48).** The `schema.snap` migration serializer for
  multi-alternative scopes (record scopes by name + each model's alternative set — the E-track change,
  G5); facts/LSP go-to-def/rename/hover over scope refs + the D4/D5-hover-scrub. The shard key stays the
  model's first scope term (D33); a per-callable-alternative shard key is not needed until multi-owner
  routing is exercised.

## D50 — scope editor surface + `schema.snap` scope serializer + UI decision-ref scrub
Implements D46/D47's iteration 3 (Track G, G5): the facts/LSP surface for named scopes, the migration
snapshot serialization of scopes, and the user-directed scrub of internal decision-record refs from every
editor-facing string. Closes Track G. Rename across scope refs stays deferred to the C4 rename iteration
(it needs the full reference-site index C4 builds).
- **Go-to-definition on scope refs (`based-lsp`).** `collect_scope_refs` collects every scope-name
  reference ident — `@scope Name[, …]` on a model, `scoped Name[, …]` on a query/mutation — as the
  reference-collection twin of the D43 `collect_type_refs`. `Snapshot::definition_at` first tries a
  model/shape type ref (→ that decl's name), then a scope ref (→ the `scope Name (…)` decl's name span),
  routed to the file's owning project like every other position request. No new LSP capability — the
  D43 `definition_provider` already covers it. Rename is *not* built (deferred, C4).
- **Scope hover (`based-facts`).** A new `FactKind::Scope`: `scope_facts` emits one span-anchored fact at
  the `scope` decl's name and at every `@scope`/`scoped` reference, so hovering any of them explains the
  contract. The detail is self-contained — the scope name, its `col = $ctx.field [and …]` filter, the
  models it governs, and the `scoped …` / `unscoped("reason")` opt-in — no decision-record refs. It is
  hover-only: the LSP `inlay_hint` skips `FactKind::Scope` (a scope is *written*, not a derived fact, so
  it needs no inlay). Consumed by `based facts` + LSP hover unchanged.
- **`schema.snap` scope serialization (`based-codegen::migrate`, extends D39).** The snapshot now records
  named scopes: `Snapshot.scopes: Vec<ScopeDeclSnap>` renders as top-level `scope <Name> (<col>: <Type>
  = $ctx.<field>, …)` lines (sorted by name, before the tables — the one place a scope column's/`$ctx`
  field's type lives), and each `TableSnap` carries `scope_alts: Vec<Vec<String>>` (the DNF), rendered as
  one `scope=(A, B)` header group per `@scope` alternative (commas = AND within a group, separate groups =
  OR). Both round-trip (`render`/`parse`), stable-ordered (names sorted within an alternative, alternatives
  sorted). A scope emits **no DDL** (it is an injected filter in generated code), so a scope change surfaces
  as `Step::ScopeChange(ScopeChange::{Add,Drop,Alter,Table})` — a neutral, non-destructive step that renders
  as an `up.mig` note / an SQL comment and produces zero executable statements, but *does* advance the
  snapshot so `based migrate gen` captures a scope change and `verify`'s full-snapshot-equality drift check
  stays honest (without it, a scope-only change would deadlock: gen writes nothing, verify sees drift). A
  from-scratch `0001_init` (empty prior) emits **no** `ScopeChange` steps — the scopes ride `schema.snap`
  and each table's `scope_alts` rides its `CreateTable`, so init stays create-only (its `up.mig` still
  matches `based gen sql` from scratch, which emits no scope SQL). Commerce golden re-blessed (single-scope
  `Tenant`): gains the `scope Tenant (org: Org = $ctx.org)` line and Order's `scope=(Tenant)` by-name entry.
- **UI decision-ref scrub (user directive).** Every editor-facing string in `based-facts` (the four `Fact`
  `detail`s + the `CtxRequirement` variant doc) that referenced a decision record (`D<n>`), a principle,
  or a spec-doc filename (`.md`) was rewritten into clean, self-contained user prose. The known leak — the
  `$ctx` hover's "inferred per-callable … (D4/D5). Nothing in source declares them" — is now false (a scope
  field is *declared*) and gone: it reads "each field's type is fixed by the scope decl or the column it
  binds to." The public diagnostic codes (`E01xx`/`W01xx`) are kept — legitimate user-facing identifiers.
  A regression test (`no_editor_string_leaks_a_decision_or_principle_ref`) asserts no `Fact` label/detail
  matches a `D\d`/`P\d`/"principle"/`.md` pattern.
- **Tests.** `based-facts`: the scrub guard + a scope-hover test (4 anchors: decl + `@scope` + two
  `scoped`). `based-codegen`: a multi-alternative (OR) scope round-trip + diff test (serialize/parse/diff
  proven) + an init-omits-scope-steps test; commerce snapshot golden + round-trip updated. `based-lsp`: a
  go-to-def test resolving both `@scope Tenant` and `scoped Tenant` to the decl.

## D51 — field-reference go-to-def + broad hover + clickable inverse inlay
Track C4a (user-raised): three editor refinements the author noticed on the commerce `Order` model.
Baseline navigation/hover depth a `.bsl` author expects from rust-analyzer, built on the D43 resolver.
No new LSP capabilities — the existing `definition_provider`/`hover_provider`/`inlay_hint_provider`
already advertise all three.
- **Field-reference go-to-def (`based-lsp`).** `Snapshot::definition_at` gains a third resolver after the
  D43 type refs and D50 scope refs: `field_ref_at` matches the cursor against *field-reference path
  segments* and walks them to the field they name. Every `Path` in a shape body (`placed_by`,
  `placed_by.name`, `org.name`), a query `where`/`order` clause, and a mutation write's `where`/assign
  columns is collected by `field_paths` paired with its statically-known **root model** — the shape's
  `from`, the block statement's target (or, for inline/bare queries, the return shape's `from` / return
  model via `query_root`), and each write statement's model. `walk_path` resolves a segment prefix
  against that root, advancing through relation edges (`placed_by` → `User`) so any segment — not just
  the first — resolves, cross-file included. Filters are **out**: their root is the polymorphic call
  site (no static root), consistent with the D14 call-site re-resolution. This is the reference-site
  index find-references/rename (C4) will generalize to *all* sites of a target, not just the one under
  the cursor.
- **Broad hover ("what", `based-lsp`).** `Snapshot::hover_at` returns the *declaration* of the symbol
  under the cursor, rust-analyzer-style: a field → `name: Type` (+ a to-one/to-many relation note), a
  model/shape/scope/callable reference *or its own decl name* → a one-line signature (`model Order`,
  `shape OrderCard from Order`, `query name(params) -> Ret[]`, `scope Tenant (org: Org = $ctx.org)`).
  Reuses `field_ref_at` + the D43/D50 reference collectors, then falls back to `decl_site_hover` for a
  cursor on a declaration's own name. The LSP `hover` handler leads with this "what" and appends the
  existing derived-fact "why" (`---`-separated), so a field with an inferred inverse shows both.
- **Clickable inverse inlay (`based-facts` + `based-lsp`).** The inferred-inverse inlay was
  `inverse <- OrderItem via order` — wordy (the `OrderItem[]` type is already on the line) and inert.
  Trimmed to **`via OrderItem.order`** (model-qualified so the field name — which can echo a model
  name — reads unambiguously) and made command-clickable: `Fact` gains a `nav: Option<Span>` (the
  paired forward edge's span, resolved from the checked schema — `OrderItem.order`), and the LSP
  renders the inverse hint as an `InlayHintLabelPart` carrying that `Location`, positioned at
  end-of-line like the model/callable-wide facts (extracted into a testable `Snapshot::inlay_hints`).
  Other fact kinds keep their plain `tag + label` string. The full "why" stays on hover. **Click
  activation:** VS Code activates a label part by running go-to-def *at* its `location` (LSP 3.17), not
  by jumping to it — so the location (the forward edge's *declaration*) had to resolve, or the link
  underlines yet the click is inert. `definition_at` now resolves a cursor on any declaration's own
  name to itself (`decl_name_at`: model/field/shape/callable/scope), closing that round-trip and
  doubling as the go-to-def-on-a-definition convention.
- **Lint tone (user directive).** The `W0104` useless-index messages were reworded to lead with the
  verdict and drop internal phrasing: `index on \`x\` is unnecessary: …, so this only adds write cost
  — drop it` (was `… — pure write tax; drop it`). "Pure write tax" stays in the *internal* code
  comments (`ir.rs`/`indexes.rs`), not the user-facing string.
- **Tests.** `based-facts`: the inverse label is now `via OrderItem.order` with a non-`None` `nav`.
  `based-lsp`: field-reference go-to-def over commerce (`placed_by` → `Order.placed_by`,
  `placed_by.name` → `User.name` cross-file, a bare shape field → its column) + a hermetic
  query-`where`/`order` + mutation-assign column test; a hover test asserting field/model/shape
  signatures for both references and declaration sites; an inlay test asserting the inverse hint is a
  `LabelParts` whose location round-trips through `definition_at` (i.e. the click resolves). `based-sema`
  lints conformance golden re-blessed for the reworded `W0104`.

## D52 — find-references + filter go-to-def (`based-lsp`)
Track C4 "find references", user-raised as the "back-follow": from a forward relation edge
(`OrderItem.order`), navigate to what uses it — the inverse `Order.items` that pairs through it.
`⌘`-clicking a *declaration* makes VS Code run find-references (not go-to-def), so this needs the
`references` capability; without it the click reports "no references found". Advertised
`references_provider`.
- **`Snapshot::references_at(fid, offset, include_decl)`** — the inverse of `definition_at`: resolve
  the cursor to its target declaration span (`definition_at` already returns that for a reference *or*
  a declaration name), then collect every site that resolves to the same span. It reuses the existing
  reference collectors (D43/D50/D51) rather than a second index: model/shape type refs
  (`collect_type_refs` + `type_ref_target`), `@scope`/`scoped` refs (`collect_scope_refs`), filter
  calls (new `collect_filter_refs`, walking every predicate for `Predicate::FilterCall` names),
  field-reference path segments (`field_paths` + `walk_path`, so a field's uses across shapes / query
  `where`+`order` / mutation writes all count), the **inverse back-edge** (any `Fact` whose `nav` is
  the target field — an inferred inverse's paired forward edge — contributes its own span, giving the
  back-follow), and explicit `(Model.field)` inverse pairings. Deduped, span-ordered; the declaration
  itself is appended when `include_declaration` is set.
- **Filter go-to-def (`definition_at`).** A cursor on a `filter(...)` call now resolves to the `filter`
  decl (the callable actually referenced from other `.bsl`, unlike queries/mutations which are wire
  endpoints). The three ref resolvers in `definition_at` were factored into `type_ref_target` /
  `scope_ref_target` / `filter_ref_target` so both go-to-def and `references_at` share them.
- **Tests (`based-lsp`).** The back-follow over commerce (`OrderItem.order`'s references include
  `Order.items`, plus the decl with `include_declaration`); a hermetic filter test (go-to-def on a
  `big(5)` call → `filter big`; find-references on `filter big` → the call site; find-references on a
  field → its `where` use).

## D53 — rename + prepareRename (`based-lsp`)
Track C4 "rename", the natural pair of find-references. A rename must rewrite the symbol's
declaration and every reference that *spells its name*, across files. Advertised `rename_provider`
with `prepare_provider: true` so the editor pre-validates the token and highlights its extent before
prompting.
- **`Snapshot::rename_edits(fid, offset, new_name)`** reuses the D52 reference index rather than a
  second walk: `references_at(fid, offset, /*include_decl=*/true)` gives every resolving site, then
  each is filtered to those whose current source text equals the declaration's text and rewritten to
  `new_name` (grouped by owning file → `WorkspaceEdit.changes`). The text filter is the load-bearing
  distinction from find-references: the **inverse back-edge** (`Fact.nav`) is a *differently-named*
  field (`Order.items` pairing through `OrderItem.order`) — correct to *list* as a reference, wrong to
  *rename* — so it is excluded because it spells `items`, not the old name. All other reference sites
  (type refs, scope refs, filter calls, field path segments, explicit `(Model.field)` inverses) spell
  the target name by construction, so the filter only ever drops the back-edge. `None` when the cursor
  is on no renameable symbol (gated on `definition_at`) or `new_name` is not a valid identifier
  (`is_ident`; casing rules like models-UpperName are left to sema, which re-flags a bad rename inline).
- **`Snapshot::prepare_rename_range(fid, offset)`** returns the identifier extent under the cursor for
  prepareRename, gated on the same `definition_at` resolver so keywords / primitives / literals /
  whitespace decline. The extent is a byte walk over identifier characters (identifiers are ASCII; a
  non-ASCII byte stops the scan).
- **Tests (`based-lsp`).** Cross-file model rename (`Org` → `Organization` edits both the decl in
  org.bsl and the type ref in user.bsl; exactly the `Org`-spelled sites, none else); forward-edge
  rename over commerce leaves the `items` back-edge untouched; rejection of a non-identifier new name
  and of a cursor on a primitive keyword; prepareRename offers the declaration's identifier extent.

## D54 — workspace symbols (⌘T) (`based-lsp`)
Track C4 "workspace symbols", the project-wide counterpart to document symbols (D44). Where
`documentSymbol` outlines one file, `workspaceSymbol` (⌘T) searches *every* declaration in the
project by name, so an author jumps to any model/callable without knowing its file. Advertised
`workspace_symbol_provider`.
- **`Snapshot::workspace_symbols(query)`** walks the whole parsed decl set (not one `fid`) and emits a
  flat `SymbolInformation` per named declaration: models (Struct) with their fields (Field, nested via
  `container_name` = the model name), shapes (Interface), scopes (Namespace), queries (Function),
  mutations (Method), filters (Function). Each carries its own file `Location` resolved from the name
  span's `FileId` → `sources` → `Url`, so results span files. Kinds match the D44 document-symbol
  mapping; scopes are additionally surfaced here (⌘T is the "find any named decl" entry point).
- **Fuzzy filter.** `fuzzy_match(query, name)` is a case-insensitive ordered subsequence test (the ⌘T
  convention: every query char appears in order, not necessarily contiguously); an empty query matches
  all. Coarse server-side filter — the client re-ranks. `"oc"` matches `OrderCard`, `"co"` does not.
- **Handler (`main.rs::symbol`).** Sweeps every open project's snapshot (⌘T is workspace-wide, not
  one file). A file shared by nested manifests appears in more than one snapshot, so results are
  deduped on `(uri, start line, start char)` — identical spans across snapshots collapse to one.
- **Tests (`based-lsp`).** Over commerce: symbols span multiple files; `Order` → Struct with its
  `status` field contained in `Order`; kind mapping for shape/query/mutation; the `"oc"` fuzzy query
  narrows to a subset including `OrderCard` while a non-subsequence query drops it. Plus a unit test
  pinning `fuzzy_match` as an ordered, case-insensitive subsequence with empty-matches-all.

## D55 — nested to-one shape sub-objects (`field { … }`)
Track L1, first slice. A shape may expand a **to-one** relation into a nested object
(`shape OrderCard from Order { total, placed_by { name, email } }` → `{ total, placed_by:
{ name, email } }`). Sema already type-checked the nested body (`check.rs` `ShapeField::Nest`);
every emit surface silently dropped it. Now built end-to-end. **To-many arrays** (`items:
OrderItem[]`, the self-referential `User.invited_users`) stay deferred — they need JSON
aggregation / a companion query + self-ref join aliasing (the remaining L1 follow-up).
- **Cardinality rule.** A `Forward` relation is always to-one. An `Inverse` is to-one only
  when its paired forward FK (`via`) is unique on the target (a genuine one-to-one back edge);
  otherwise it is a collection and the nest is skipped (not mis-lowered). One helper expresses
  this on each emit surface: SQL `Select::enter_to_one`, client/OpenAPI `to_one_relation`.
- **SQL (`sql/dml.rs`).** A to-one nest reuses the *same* relation JOIN a reach-rename builds
  (join dedup keyed by path prefix, so `buyer = placed_by.name` and `placed_by { name }` share
  one join). The nested model's columns are projected under a **prefixed output alias**:
  `placed_by { name }` → `` `j_placed_by`.`name` AS `placed_by.name` ``. The separator is
  `NEST_SEP = '.'` — a `.` cannot occur in a BSL identifier, so any alias containing it is
  unambiguously a nested projection. Nesting recurses (`placed_by { org { name } }` →
  `placed_by.org.name`), deepening both the join alias and the output prefix. `project_body`
  threads `(alias, prefix, out_prefix)` and resolves paths from the nested model's alias.
- **Runtime reassembly (`run.rs`).** `nest_row` splits each flat row key on `NEST_SEP` and
  rebuilds the nested object (`{"placed_by.name": …}` → `{"placed_by": {"name": …}}`), recursing
  for deeper nests. Applied to every query envelope (`get`/`list`/`page`) and the mutation
  declared-shape re-select. A flat query has no dotted key, so the transform is a no-op there —
  it never changes existing behaviour. The alias convention is codegen's single source of truth,
  read by the runtime (principle 4).
- **Client + OpenAPI.** The client emits a nested struct `<Parent><Field>` (e.g.
  `OrderCardPlacedBy`) referenced by the parent field's type (`Option<…>` when the relation is
  optional), deduped across callables. OpenAPI emits an inline nested object schema, required
  unless the relation is optional. Both mirror the SQL cardinality rule; to-many nests are still
  skipped on both.
- **Verified live.** A self-contained SQLite integration test (`sqlite_integration.rs`) seeds a
  User + Order and dispatches a nested `get`/`list`, asserting the reassembled nested JSON against
  a real engine — not compile-verified. Plus codegen unit tests (SQL prefix aliases + recursion +
  to-many skip + Postgres quoting; client nested struct; OpenAPI nested object schema).

## D56 — keyset-cursor pagination (`page` without `offset`)
Track L2. A keyset `page` walks the whole set: each page returns the next window + an opaque cursor,
the caller passes the cursor back for the next page, and the final short page returns a `null` cursor.
Previously codegen emitted only `ORDER + LIMIT + id`-tiebreaker and the runtime hard-coded `cursor:
null`, so a keyset page returned only page 1. Now built end-to-end (offset pagination was already
complete; this is its keyset twin). Governed by pagination.md (principle 1: keyset is the safe default;
principle 8: the tiebreaker is shown, not written).
- **Cursor comparison (`sql/dml.rs`).** The cursor `WHERE` is the lexicographic "strictly after the
  cursor" predicate over the ordered sort keys: `(k0 ▷ v0) OR (k0 = v0 AND k1 ▷ v1) OR …`, where `▷`
  is `>` for an ASC key and `<` for a DESC key. The **expanded** form (not a `(k0,k1) > (v0,v1)`
  row-value comparison) is used because SQL row comparison can't mix ASC/DESC directions and the
  expansion is portable across all three dialects. The predicate is guarded — `(:keyset_active = 0 OR
  (…))` — so page 1 (no cursor, `:keyset_active = 0`) short-circuits it to a no-op and the `:keyset_<i>`
  placeholders bind to NULL (never consulted). One fixed SQL string, always the same placeholders — no
  string surgery per page.
- **Hidden cursor basis.** The sort keys may not be projected (the shape can omit `created_at`/`id`),
  so the SELECT carries them a second time as `<key> AS __keyset_<i>` columns. The runtime reads the
  last row's `__keyset_<i>` to mint the next cursor, then strips the `__keyset_` columns from the
  response. The `__` prefix can't begin a BSL identifier, so it never collides with a field.
  `KEYSET_PREFIX`/`LoweredQuery.keyset` (the key count) are codegen's single source of the convention
  (principle 4), read by the runtime.
- **Deterministic order.** A non-offset `page` **always** gets the unique `id` tiebreaker appended
  (unless the sort already ends on `id`) — even with no explicit `order`/`@sort`, so an empty order
  still yields `ORDER BY id` and the cursor has a unique basis that never drops or repeats a row.
  Offset pages don't get it (their window is positional).
- **Opaque cursor (`based-runtime/cursor.rs`).** Wire form `<fnv-checksum-hex>.<payload-hex>`, payload
  = the JSON array of the last row's sort-key values. Decode validates the checksum, JSON structure,
  and arity; any deviation is `PlanError::BadCursor` → 400 (never fed to the query). The checksum
  catches corruption/tampering; it is **not** a cryptographic signature (that needs a server secret —
  deferred). The real safety property — no predicate injection — holds regardless: cursor values only
  ever fill bound parameters, never concatenate into SQL. The count query (`with count`) stays
  cursor-free (page-independent total), so the guard lands on the main `WHERE` only.
- **Client + OpenAPI.** A keyset query's input gains `cursor: Option<String>` (absent = first page);
  an offset query's gains `offset: Option<i64>` (this also completes offset's previously-missing input
  surface). `Page<T>.cursor` (client) + the OpenAPI page envelope already modeled the returned cursor.
- **Verified live.** A self-contained SQLite integration test pages a `page (2)` query through 5 rows
  (full → full → short), asserts the cursor works even though the sort basis is unprojected (hidden
  columns stripped), and asserts a tampered cursor → 400. Plus codegen unit tests (the guarded
  comparison, `__keyset_` columns, offset-emits-no-keyset) and `cursor.rs` round-trip/tamper/arity
  unit tests.

## D57 — to-many nested arrays in shapes (`items { … }`, self-referential `invited_users`)
Track L1, final slice — closes L1 (to-**one** nests landed in D55). A shape may expand a
**to-many** relation (an Inverse collection edge) into a nested **array** of sub-objects
(`shape OrderCard from Order { total, items { sku, qty } }` → `{ total, items: [{ sku, qty },
…] }`), including the flagship self-referential `shape UserCard from User { name, invited_users
{ name } }`. Sema already type-checked the nested body (D55's `check_shape_body` Inverse
branch); every emit surface skipped a to-many nest cleanly (guarded by the cardinality check).
Now built end-to-end across all three dialects. Governed by shapes.md (nesting = brace block)
+ relations.md (inverse collections); principle 4 (one alias convention, codegen owns it,
runtime reads it).
- **SQL: correlated subquery, not a join (`sql/dml.rs`).** A to-many nest lowers to a scalar
  **correlated subquery** in the SELECT list: `(SELECT <json-agg>(<json-object of the element
  body>) FROM <child> AS <s-alias> WHERE <child.back_fk> = <outer>.id AND <child soft-delete /
  @scope>)`. A subquery (not a `LEFT JOIN` + `GROUP BY`) is used because it never multiplies the
  outer rows, needs no grouping over every projected column, and composes with `LIMIT`
  pagination unchanged. The output alias carries the [`ARRAY_MARK`] `[]` suffix (`items[]`) — a
  sibling convention to D55's `.` [`NEST_SEP`] — so the runtime knows to parse the column's
  string as a JSON array; `[`/`]` cannot occur in a BSL identifier, so it never collides.
- **Per-dialect JSON aggregation (the `Dialect` seam).** `Dialect::json_object_fn` +
  `json_array_agg`: SQLite `json_group_array(json_object(…))`, MariaDB
  `COALESCE(JSON_ARRAYAGG(JSON_OBJECT(…)), JSON_ARRAY())`, Postgres
  `COALESCE(json_agg(json_build_object(…)), '[]'::json)`. The coalesce yields `[]` for a
  childless parent (MariaDB/Postgres aggregate NULL over zero rows; SQLite already yields `[]`,
  so it's a harmless no-op there). Reuses the existing quoting seam; nothing else branches.
- **Self-ref aliasing.** Each to-many subquery mints a distinct child root alias `s<n>_<table>`
  from a monotonic `Select.sub_counter`, so a self-referential edge (`User.invited_users` joined
  to `User`) never collides with the outer `user` row — the correlation `s1_user.invited_by_id =
  user.id` is unambiguous. The counter threads through nested subqueries so siblings stay unique.
- **Recursive element builder.** `Select::json_object_expr` builds a JSON object for the element
  body over a *fresh* sub-`Select` (its own join scope, so reaches/to-one nests inside an element
  accumulate their joins into the subquery, not the outer SELECT): bare/reach fields → `'key',
  <col>` pairs, a to-one nest → a nested `json_object`, a to-many nest → a nested correlated
  subquery. Nesting composes to any depth (to-many-in-to-one aliases as `parent.items[]`).
- **Runtime parse (`run.rs`).** `nest_row`/`insert_path` gained an array-leaf case: a key ending
  in [`ARRAY_MARK`] is a to-many array — its value (a JSON-array *string* from the driver, or an
  already-decoded array, or NULL) is normalized to a JSON array and stored under the field name
  without the marker. The element sub-objects arrive fully formed (their own nesting done by the
  SQL JSON functions), so no further reassembly is needed inside the array. A flat query has no
  such key, so the transform stays a no-op there.
- **Client + OpenAPI.** The client emits the element struct `<Parent><Field>` and the parent
  field takes `Vec<…>` (deduped like the to-one nested structs); OpenAPI emits an `array` schema
  whose `items` are the element object schema, always `required` (empty array when childless).
  Both mirror the SQL cardinality rule via a `to_many_relation` helper (Inverse, paired FK not
  unique) — the twin of the SQL side's `to_many_edge`.
- **Array element order is unspecified.** Portable JSON aggregation offers no cross-dialect
  ordered form (MySQL/MariaDB `JSON_ARRAYAGG` takes no `ORDER BY`; only Postgres/newer SQLite
  do), so the array is treated as a **set**. An ordered form is a future refinement if a use case
  needs it; documented rather than silently order-dependent.
- **Verified live.** Self-contained SQLite integration tests (`sqlite_integration.rs`) seed an
  Order with items (one soft-deleted → excluded) + a childless order (→ `[]`) and the
  self-referential `invited_users` (Ada invited Bob + Cy → nested array; a leaf → `[]`),
  dispatching real `get`/`list` requests through the engine and asserting the reassembled nested
  JSON — not compile-verified. Plus codegen unit tests: the MariaDB + Postgres aggregation SQL,
  the `s<n>_` self-ref alias, the `items[]` output alias, subquery-not-join; the client `Vec<Sub>`
  struct; the OpenAPI array-of-object schema.
