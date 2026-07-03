# decisions.md

Implementation decisions the prose specs left open. Each resolves an ambiguity the
grammar and compiler must commit to. Governed by `principles.md`; where a default is
chosen it follows principle 2 (omission must have one safe meaning).

Status: proposed. These are my reads of the spec — flag any you'd decide differently.

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
from shape query mutation filter unindexed unsafe on column table by has in not and or
true false null now`.

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

**OPEN — do NOT implement `@scope` injection until this is resolved.** The feature is useful
but must not land in an uncomfortable middle ground where it is neither the common case nor a
clean edge case. Axes to settle first:
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
- **Shard-key source deferred, decoupled from D19.** The natural shard key is the tenant/owner —
  the same field `@scope` would use — but `@scope` injection is OPEN (D19), so the key extractor
  is left pluggable and **not** yet bound to a `$ctx.<field>`; today it takes an explicit key.
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
