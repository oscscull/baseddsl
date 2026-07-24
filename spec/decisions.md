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
  D9 (free layout), D82 (enum type: string + numeric kinds, explicit values, column + CHECK,
  name-resolution disambiguation, variant navigation), D83 (decimal + float: exact `decimal(p,s)`
  string-on-wire via `rust_decimal`/`serde-str`, `float` = f64, numeric family, `Literal::Decimal`
  exact-text defaults, SQLite TEXT storage, Postgres binary-numeric decode, E0159),
  D103 (implicit `id` → explicit-in-source, error `E0261` + autofix), D104 (opaque column types
  via `raw("…")` — literal type-string in DDL+snapshot, opaque client value; `sql` banned as syntax)
- **Manifest & discovery** — D5 (project manifest + `**/*.bsl` glob)
- **`$ctx` (per-request context)** — D4 (inferred, never a global type)
- **Scope / auth** — D19 (`@tenant` removed; `@scope` open), D32 (`@scope` resolved: single-owner
  filter + create auto-set + `unscoped`), D33 (shard key ← scope `$ctx` field), D34 (`@scope` in a
  joined `ON`), D46 (named scope, spec), D47 (multi-scope DNF, spec), D48 (named scope, impl),
  D49 (multi-scope DNF, impl + E0186), D50 (scope editor surface + snapshot serializer),
  D80 (`$ctx`-field rename; scope column left alone as a polymorphic contract name),
  D81 (`@scope` confines a nest-only scoped child: both shape walks recurse into `Nest`/`NestRef`,
  E0185 at compile time + runtime enforcement kept; type optionality mirrors the schema only),
  D88 (Handle-3 `guard` runtime seam: registered host async fns, deny → 403 `guard_denied`,
  unregistered fails at engine build / listener startup), D95 (guard re-entry is safe:
  id minting is `&self`/internally synchronized, dispatch holds no engine-wide lock —
  a guard may call the typed client over its own engine), D99 (the re-entry handle is
  first-class: `GuardRequest::engine()` hands the dispatching engine to the guard, so a
  state-reading decision goes through the schema's own scoped/soft-deleted queries — no
  `OnceLock` back-reference, no hand-written SQL; `Engine` is now a `Clone` handle)
- **SQL codegen — DDL** — D10 (type mapping)
- **SQL codegen — query reads** — D11 (query SELECTs), D14 (named-filter body resolution),
  D93 (`in` value-list form: `Predicate::InList`, per-element sema check, `IN (v, v, …)` lowering),
  D94 (whole-query raw bodies: `{ sql`…`; }` is the statement — param-binding + shape-typed
  results survive, W0102 lints the soft-delete gap, un-composable combinations rejected E0210–E0214),
  D96 (raw marker renamed `sql` → `raw` — one spelling with the feature name and the `.mig`
  `raw(dialect)` step; no alias)
- **SQL codegen — mutations/writes** — D12 (mutation writes + create-keyed re-select), D16 (tx
  back-refs `^`), D58 (update/delete/restore where-keyed declared-shape re-select + delete-shape
  resolution), D92 (zero-row surviving-write mutation → 404 `not_found` + rollback, never a null
  success), D98 (`-> ok` shapeless ack return for real DELETEs; shape-on-real-DELETE E0220,
  ack-on-surviving-write E0221, ack-on-query E0222; zero-row DELETE → 404 `not_found`),
  D100 (atomic update expressions: `update … { total = total + $n }` — a self-referential
  arithmetic SET over the model's numeric columns/params, lowered to real SQL `SET col =
  (…)`, never read-modify-write; `+ - * /`, numeric family only, update-only; E0230/E0231),
  D102 (upsert: `create … on conflict (target) update { … }` — per-dialect `ON CONFLICT DO
  UPDATE` / `ON DUPLICATE KEY UPDATE`, conflict-key-keyed read-back; E0250-E0254),
  D107 (named `tx` step bindings `create … as name;` + `$name.field`, reaches any prior step;
  **`^` removed entirely**, `E0170` retired; E0280/E0281 — supersedes D16)
- **Indexing** — D15 (index inference, baseline emission, lints), D103 (inferred join-key indexes
  retire → explicit `@index`, error `E0260` + autofix; principle 8 reworded; `inf_`/`IndexSnap.inferred`
  gone), D104 (exotic indexes: `@index(col) using <method>` + opaque `@index raw("…")`, per-dialect
  validity `E0272`/`E0274`)
- **Relations** — D17 (custom `on:` join resolution), D102 (many-to-many via explicit
  junction model — two forward edges + two to-many inverses, no new syntax; far-side
  flattening + implicit-junction sugar deferred), D103 (m2m fork resolved: **no** implicit-junction
  sugar — a junction's FK columns are explicit `@index`), D109 (far-side flattening projection
  `courses = enrollments.course { … }` → a flat distinct `Vec<Course>`, junction hidden;
  two-level IN-subquery lowering, scope/soft-delete on junction+far, E0300–E0302 — closes T5;
  implicit-junction sugar stays rejected), D108 (opt-in FK referential actions: `@fk(…)` /
  `@no_fk` + toml `[schema] foreign_keys` convention + the divergence-reason rule; E0290–E0295, W0110)
- **Aggregations** — D101 (aggregations + group by + having: an *aggregate shape*
  `count()`/`sum`/`avg`/`min`/`max` projected over groups, paired with a query's
  `group by`/`having`/`order`; `GROUP BY`/`HAVING` lowering with row filter before grouping +
  deterministic decode casts; result typing count→int, sum/min/max→Option<col>, avg→Option<float>;
  boundaries E0240–E0245)
- **Query / shape codegen** — D11 (SQL DML mapping), D55 (nested to-one shape sub-objects),
  D57 (to-many nested arrays: correlated-subquery JSON aggregation + self-ref aliasing),
  D79 (named nested projection: `field -> Shape` references a shape decl — same SQL as inline,
  one nominal client/OpenAPI type; E0132/E0133/E0134), D87 (ordered to-many nests: relation
  `@sort` > child model `@sort` as ORDER BY inside the JSON aggregate, all three dialects —
  supersedes D57's order-unspecified caveat), D109 (far-side flattening projection
  `out = edge.far { … }` — a two-level correlated IN-subquery skipping a m2m junction to the
  distinct far side; reuses D57's json-agg + `field[]` marker, runtime unchanged; E0300–E0302)
- **Pagination** — D56 (keyset-cursor pagination: lexicographic `WHERE`, hidden cursor-basis columns,
  opaque validated cursor), D59 (keyset + offset proven live on MariaDB/Postgres), D85 (streaming is
  the non-paginating full pass; `page` on a stream query is E0201), D97 (`Page<T>.total: Option<i64>`
  — `Some` exactly for a `with count` query; OpenAPI advertises `total` only on a counted page schema)
- **Streaming reads** — D85 (Track N2 design: `-> stream Shape` signature form, NDJSON wire with a
  mandatory terminal `done`/`error` line + truncation-is-failure contract, same-named
  `Stream`-returning client method with two-layer `Result`, drop = cancel, per-row nest
  materialization; E0200/E0201/E0202)
- **Client codegen** — D13 (typed Rust client), D30 (typed per-callable `$ctx` in the client), D62
  (emitted in-process embedded bridge `client::embedded(&engine)`, opt-in via `ClientOptions`; inner
  `#![allow]` wart fixed), D63 (`based gen client --embedded` CLI flag), D70 (typed ids: per-entity
  phantom `Id<entity::M>` newtypes, transparent wire, `from_raw` escape, no blanket `From<String>`),
  D71 (production-grade errors: structured `ClientError` + `std::error::Error`/`Display` on runtime errors),
  D73 (typed `Cursor` newtype in the paginated surface: single opaque `#[serde(transparent)]` type,
  `from_raw` escape, no blanket `From<String>`), D79 (a `field -> Shape` nest emits the named shape's
  struct/schema instead of a per-parent anonymous type), D85 (a `-> stream` query's method returns a
  typed row `Stream`; `Transport` gains a streaming call), D88 (`<name>_with_key` mutation twins +
  `Transport::call_with_key`: the `Idempotency-Key` header / `Engine::call_with_key`, emitted only
  when the schema declares a mutation), D97 (`Page<T>` gains `total: Option<i64>`, skipped when
  serializing `None` so a re-served page mirrors the engine wire), D98 (an `-> ok` mutation's
  method returns `Result<(), ClientError>` via a shared empty `Ack` decode type; OpenAPI `Ack`
  component — both emitted only when the schema declares an ack mutation)
- **Errors** — D71 (`ClientError` kind/code/status + `Error`/`Display`/`source()`; `PlanError`/`DbError`/
  `RunError` implement `Error`+`Display` with stable `code()`; single source of truth shared by the wire),
  D72 (CLI: structured `CliError` + `Display`/`source()` chaining + exit-code convention — usage 2 /
  failure 1; reuses D71's typed errors as the cause instead of re-stringifying; `anyhow` dropped),
  D75 (HTTP listener edge `EdgeError` registry: shared `code()`/`status()`, pool checkout reuses the
  driver's classified `DbError::code()`), D76 (example `main.rs` as a `?`-based error-handling reference
  matching on `ClientError::kind()`/`code()`), D92 (`RunError::NotFound` → 404 `not_found`: a
  surviving-write mutation whose `where` matched no row)
- **Polyglot / OpenAPI** — D23 (OpenAPI over gRPC, rationale), D24 (OpenAPI emitter shape)
- **Runtime architecture** — D18 (in-process, not artifact-consuming), D20 (serving model: sync +
  bounded pools, single-shard scale-out — execution model superseded by D84), D22 (in-process `embed`
  door), D25 (write-retry idempotency), D26 (health/readiness + graceful shutdown), D31 (idempotency
  key fingerprint), D65 (live-DB hardening: statement timeouts, bounded deadlock-retry,
  pool-exhaustion→fast-503), D84 (async-native execution architecture: sqlx driver layer, typestate
  tx, stream-first reads, enforced coloring boundary — Track N0 design), D88 (guard registry on
  the engine; single dispatch enforcement point on both doors), D95 (`IdGen` mints by
  `&self` — the engine wraps the generator in no lock, so no call path holds anything
  across dispatch's awaits; guard re-entry deadlock made unrepresentable)
- **HTTP listener** — D21 (`based serve` + multi-dialect readiness)
- **Dialects & drivers** — D27 (SQLite backend), D28 (SQLite DDL), D29 (Postgres dialect + `$n`
  scanner), D38 (Postgres driver + live suite), D61 (Postgres binary-format result decode: uuid/
  timestamptz/date/jsonb columns → canonical strings), D65 (per-dialect statement-timeout + deadlock/
  pool-exhaustion classification via `DbErrorKind`), D84 (drivers unify on sqlx as the executor/pool
  layer — the hand-rolled `mysql`/`postgres`+r2d2/rusqlite driver stacks retire in N1)
- **Testing / integration harness** — D35 (Docker-backed real-DB harness + MariaDB live suite), D59
  (live pagination + soft-delete/restore coverage on MariaDB/Postgres; Postgres numeric text-bind fix),
  D64 (`TEST_*_URL` env override: live suites connect to a provided server instead of self-spinning),
  D65 (live hardening tests: statement-timeout abort, crossed-lock deadlock, fast pool-exhaustion)
- **CI / deploy (keep-proven)** — D64 (Track D2 CI: portable `make` targets + `.github/workflows/ci.yml`
  thin example wrapper; env-URL override, readiness-wait, migrate-apply/examples/extension in CI),
  D66 (Track D1: `based serve` container image — dialect-aware serve + env config + `docker/` + `ci-image`)
- **Editor / LSP** — D36 (VS Code thin LSP client), D40 (per-file manifest resolution), D43
  (go-to-def + type coloring), D44 (document symbols + capability audit), D45 (completion),
  D51 (field-reference go-to-def + broad hover + clickable inverse inlay), D52 (find-references +
  filter go-to-def), D53 (rename + prepareRename), D54 (workspace symbols ⌘T), D67 (offline
  migration-drift diagnostic W0108 + spent-`@was` W0107), D68 (folding + selection ranges — Track
  C4 feature-parity complete), D77 (editor gravy names symbols: trimmed scope/`$ctx` hovers +
  dropped duplicate `$ctx` inlay), D78 (`based fmt` canonical formatter + `formatting` LSP directive —
  one printer shared by CLI and editor), D80 (comprehensive rename: params, `$ctx` fields, callable
  names, `@was`-aware physical rename), D90 (signature param bindings are field references:
  go-to-def/find-refs/rename see `-> edge` / `op col` idents; binding hover states the generated
  predicate; tmLanguage keyword/type audit), D91 (derived facts anchor narrowly — inferred-index
  fact at the inducing forward member, callable ctx/resolved-query facts at the name ident — so
  hover facts no longer bleed across the whole decl)
- **Formatter / tooling** — D78 (`based-fmt` canonical `.bsl` printer: AST pretty-print + verbatim
  comment reattachment; deterministic/idempotent; `based fmt [--check]` + LSP `formatting`)
- **Migrations** — D37 (migration generation, spec), D39 (snapshot + diff engine), D41 (per-dialect
  renderer), D42 (apply + `_based_migrations` ledger), D67 (`@was` renames + offline drift diagnostic
  + `raw(dialect)` up step — Track E5, DoD #5 fully met), D105 (`@was` lifecycle: `gen` self-consumes
  the spent `@was` from source + teach-at-checkpoint rename hint in gen/`W0108`/destructive-gate;
  editor-rename insert already D80), D106 (`up.mig` snapshot-authoritative: honest header +
  apply-time drift **refusal** `MigrateError::UpMigDrift` + multi-line `raw` blocks + prefilled
  `down.mig` + `.mig` grammar + raw/snapshot boundary doc + `W0109`)
- **Example projects** — D60 (`examples/` outside the workspace; SQLite quickstart: build-time
  codegen + in-process `Engine` + typed client, end-to-end scenario), D61 (MariaDB + Postgres
  quickstart slices against live Docker servers; Track B / DoD #2 complete), D63 (quickstart DX
  rebuild: `based migrate apply` setup + checked-in `src/client.rs`/`migrations/` + `client::embedded`
  + `.env`/dotenvy + client-`create` seeding; no build.rs, no raw SQL; `based gen client --embedded` flag),
  D86 (Track N3 design: flagship `examples/axum-helpdesk` — multi-tenant support desk on Postgres,
  BYO-pool embedded engine, bearer→`$ctx` middleware via an `unscoped` session query, total-feature
  coverage map + syntax-appeal audit verdicts — zero grammar changes; prereqs: ordered to-many nests,
  `guard` runtime seam, typed-client idempotency key), D89 (Track N3e: final syntax-appeal re-audit
  on the shipped helpdesk artifact — zero grammar changes upheld; in-situ verdict list; final
  coverage-map deltas vs D86 — no `in` value list, raw-at-leaves instead of whole-query raw, purge
  route omitted, three additive triage mutations; library gaps promoted to PLAN items)
- **Source hygiene / conventions** — D69 (Track F1 comment-hygiene sweep: source reads as finished, no
  build-time/WIP narration, TODOs live in the roadmap `.md`s — the standing Conventions rule enforced),
  D74 (H4/H5 hygiene: positive framing over define-by-negation; `based-codegen` D#-refs + overlong
  comments cleaned; userland surfaces D#-free), D77 (editor gravy names symbols not the system:
  trimmed scope/`$ctx` hovers, dropped duplicate `$ctx` inlay — H4 complete)

## D1 — `Id` type, default PK = uuid
`Id` is a primitive scalar: the opaque primary-key type. The concrete column type of the
implicit `id` is **`uuid` by default** (distributed-friendly, non-enumerable; MariaDB native
`UUID` where available, else `BINARY(16)`).
- A model whose key is something else declares it explicitly (deviation visible, principle 2).
- `Id` is the type of the implicit `id` column and of any relation's foreign-key value.
- In query params, `org: Id` means "the key of the referenced row." Same-name binding
  to a relation field (e.g. `org`) compares against that relation's FK column.

## D2 — `id` implicit; timestamps are decorated, never implicit
> **Revised by D103 (NF11):** `id` is no longer implicit — a model that declares no `id` is
> error `E0261` (a PK is load-bearing, so principle 2 governs its omission), fixed by writing
> `id: Id`. The rest of this entry (timestamps decorated, never implicit) stands.

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
> **Revised by D103 (NF11):** the inferred join-key baseline is retired — a traversed join
> key (and a scanning root filter) with no covering `@index` is now error `E0260`, not a
> silent auto-index; `W0103` is gone (promoted to `E0260`). `W0104`/`W0105` and the
> `unindexed(…)` opt-out stand. A written `@index` on a soft-delete model keeps the
> predicate-leading rendering described below.

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

## D16 — tx back-references (`^`, mutations.md) — SUPERSEDED by D107
> **Superseded (D107).** `^` is removed; a `tx` step is bound with `create … as name` and
> referenced as `$name.field`, reaching any prior step. The lowering below (distinct
> `:id_<step>` binds, `id`-reuse) is retained, now name-addressed. Kept for that lowering history.

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

## D58 — update/delete declared-shape re-select (where-keyed)
Track L3, the last open Track L gap. Generalizes D12's re-select: a mutation now reads its written
row back in its **declared shape** even when it only *updates / soft-deletes / restores* the row
(not just when it *creates* it). Before, a pure `update`/`delete` returned `{ id }`/`{}` instead of
its declared shape (`based-codegen mutations.rs`, `based-runtime run.rs`). D12 deferred this as
"cardinality-ambiguous"; resolved here by keying the re-select off the write's own `where` and
returning the (first) matching row's shape — mirroring how a create's re-select and `get` both take
`.next()`.
- **Two key forms, one `ret_select` (codegen).** `lower_mutation` picks the re-select key: a
  **create-keyed** form (D12) when a write generates the return model's engine `id`
  (`WHERE id = :result_id`), else a **where-keyed** form (D58) — the first `update` / soft `delete`
  / `restore` on the return model, keyed on *that write's own `where`* (`SELECT <shape> FROM <model>
  WHERE <write-where> [AND <live>] AND <scope>`). Both reuse the read side's `project_return`
  (principle 4), so the projection can't drift from a `get` — nested to-one sub-objects (D55) and
  to-many arrays (D57) work in a re-select exactly as in a query.
- **Runs after the write, inside the tx (runtime).** `plan_mutation` binds the re-select whenever
  codegen emitted one; a where-keyed re-select needs no `:result_id` — it reuses the write's
  already-bound params/`$ctx` from the value environment. `apply` executes the writes then fetches
  the re-select under the same transaction (read-your-writes: an update's re-select sees the *new*
  values).
- **The live predicate rides selectively.** For `update`/`restore` the row is live afterwards, so the
  soft-delete live predicate (`deleted_at IS NULL`) is injected as in a normal read. For a soft
  `delete` the row is now **tombstoned**, so the re-select drops the live predicate (else it would
  read back as absent) — it still applies `@scope`. This is the read-side twin of the write-side
  live/tombstone injection.
- **Delete-shape resolution (documented).** A re-select can only read a **surviving** row. A soft
  `delete` tombstones (row survives → read it back without the live predicate). A **real DELETE** — a
  plain-model `delete` or a `hard delete` — physically removes the row, so there is no surviving row
  to project: such a mutation emits **no** re-select and the response falls back to `{}` (a pre-write
  capture would require reordering the write pipeline and buys little for the loud, destructive
  opt-out `hard delete` already is). A mutation returning a shape but performing a real DELETE thus
  returns `{}`; the `-> Shape` is honored only when the row survives.
- **Cardinality.** An update/delete `where` may match a set; the mutation's declared return is
  singular (`-> OrderCard`), so the re-select returns the *first* matching row (or `null` if none) —
  the same `.next()` a create's re-select / a `get` uses. Keying off the (post-write) `where` is
  correct whenever the write does not mutate a column its own `where` filters on — the norm, since
  updates are keyed on `id` (immutable); an update that rewrites its own filter column reads back as
  absent, an unusual pattern left documented rather than special-cased.
- **Incidental fix (SQLite UPDATE `SET`).** Surfacing an update through the live SQLite suite (none
  existed) exposed that `set_lhs` emitted a table-qualified `` `t`.`col` `` in `SET`, which SQLite
  rejects (it has no inline-join UPDATE, so the target is unambiguous). `SET` now emits the bare
  column on SQLite as it already did on Postgres; MySQL/MariaDB keep the qualifier (a multi-table
  UPDATE may need it).
- **Verified live.** A self-contained SQLite integration test (`sqlite_integration.rs`) seeds a
  User + Order (`status='pending'`), dispatches `set_status` (an `update` returning `OrderCard {
  status, total, placed_by { name } }`), and asserts the response is the **full** declared shape with
  the **new** status + the nested buyer (read-your-writes, one tx) — not `{ id }` — then a fresh
  `get` confirms the write committed. Plus codegen unit tests: where-keyed update re-select, soft-
  delete re-select without the live predicate, and a real delete emitting no re-select.

## D59 — live pagination + soft-delete/restore coverage on MariaDB/Postgres (+ Postgres numeric text-bind fix)
Track A4, DoD #1's remaining live-DB coverage that Track L unblocked. The keyset/offset pagination
(D56) and soft-delete/restore read-back (D58, soft-delete.md) were proven live only on SQLite; this
extends the **MariaDB and Postgres** Docker suites to prove them against real servers too, so DoD #1's
per-DB coverage is symmetric across all three dialects.
- **What was added (both live suites).** Three tests each in `mariadb_integration.rs` +
  `postgres_integration.rs`, mirroring the SQLite live keyset test: (1) **keyset** paging a `page (2)`
  query full→full→short (each full page mints an opaque cursor, the short last page a `null` cursor,
  the hidden `__keyset_*` sort-basis columns stripped from the response) with a tampered cursor →
  400; (2) **offset** paging `page (2) offset` full→full→short (the client-supplied `offset` binds
  into `LIMIT … OFFSET …`; an offset page envelope carries a `null` cursor — offset is not keyset);
  (3) **soft-delete + restore** — a soft `delete` rewrites to `deleted_at = now()` and reads the
  tombstoned row back in shape (D58 drops the live predicate for a soft delete), the row then vanishes
  from a live `list`, and `restore` clears the tombstone and reads it back live. Each suite grew a
  `compile`/`live_schema` helper that lowers a small self-contained in-line schema for its dialect and
  creates its tables from the generated DDL (so only the tested behaviour varies). Schemas declare an
  explicit `id: text` (D2) so fixtures use plain string ids rather than uuids (`VARCHAR(255)` PK on
  MariaDB, `TEXT` on Postgres — both valid, unlike a `TEXT` PK which MariaDB rejects).
- **Real bug fixed: Postgres numeric binds are now text-format.** The keyset guard emits
  `:keyset_active = 0`; Postgres infers the parameter's type from the bare literal `0` as **int4**,
  but the runtime bound `keyset_active` as an i64 encoded in **binary** (8 bytes) → `22P03: incorrect
  binary data format in bind parameter`. Root cause: `PgValue`'s `Int`/`Float` encoded in binary,
  whose width must match the inferred OID exactly. Fix (`based-runtime/src/postgres.rs`): encode
  `Int`/`Float` in **text format** (like the existing string-family mapping), so the server coerces
  the decimal string into whatever width it inferred (int2/int4/int8/numeric/float) — the same
  literal-coercion path `'…'::uuid` takes. `bool`/`null` keep binary (no width ambiguity). This is a
  general driver-robustness fix (any parameter compared to an untyped integer literal would have hit
  it), not a keyset special-case. Unit test updated + a decimal-text encoding test added.
- **Gate.** Both live suites run **green** against real `mariadb:11.4` + `postgres:16` (OrbStack
  Docker), 10 tests each; the harness still skips cleanly with no daemon. `cargo test --workspace
  --all-features`, `fmt --check`, `clippy` all clean. This closes A4's pagination + soft-delete/restore
  live coverage; A4's remaining hardening items (statement timeouts, deadlock-retry, pool-exhaustion →
  503 under load) stay open.

## D60 — Example projects: `examples/` outside the workspace; the SQLite quickstart (Track B, DoD #2)
The copyable example projects (DoD #2) live under **`examples/`** as **standalone crates deliberately
excluded from the cargo workspace** (root `Cargo.toml` `exclude = ["examples"]`), so `cargo test
--workspace` never builds them: they carry their own heavy deps (bundled SQLite) and run as end-to-end
smoke *binaries*, not unit tests. Each depends on the in-repo engine crates **by path** (so it always
tracks the current engine) and builds into its own gitignored `target/`.

**`examples/sqlite-quickstart` (the SQLite slice).** The reference a user copies to start: a reduced
commerce `.bsl` schema (`schema/`) consumed through the **generated typed client running over the
in-process `Engine`** against a live bundled-SQLite database — no socket, no server, no infra.

- **Consumption surface = typed client + `Engine`, not raw `dispatch`.** The generated `Client<T>` is
  driven by a ~20-line `InProcess` `Transport` bridge (the whole of what an embedding app writes; the
  `Transport` trait is defined *by* the generated client, so the orphan rule keeps the impl in the
  consumer crate). This is the documented Tier-1 in-process door (`embed.rs`), now shown against a real
  `SqliteDb` instead of a `MockDb`. The engine is built over one `rusqlite::Connection` (open → run the
  generated DDL → `SqliteDb::new` → `Engine::new`); SQLite is embedded, so depending on `rusqlite`
  directly to open the connection is honest, not a leak.
- **No checked-in generated code — `build.rs` regenerates it.** The build step runs the compiler front
  end as a library (`based_runtime::Compiled::load` — the same discover→parse→check `based check`/`based
  serve` use) and emits the typed client (`based_codegen::client::client`) + SQLite DDL
  (`based_codegen::sql::ddl`) into `OUT_DIR` on every build, so neither can drift from the schema; a
  broken schema fails the build with diagnostics. `main.rs` `include!`s the client and `include_str!`s
  the DDL. (The generated client's inner `#![allow(dead_code)]` is stripped in `build.rs` and re-added
  as an outer attribute on `mod client`, because `include!` rejects inner attributes on a fragment.)
  Chosen over checking in `based gen client` output (the `embed.rs` convention) because build-time
  generation can never go stale and needs no `based` binary on `PATH` — cleaner for a copyable start.
- **Scenario (runs + asserts on `cargo run`, exits 0 only if green).** create (`place_order`) →
  read-your-writes in the declared `OrderCard` shape incl. a nested `placed_by { name, email }` to-one
  sub-object → get (`order_by_id`) → list under `@scope Tenant` (`my_orders`; a different org sees none)
  → keyset pagination (`recent_orders`, `page (2)`, cursor walk) → soft-delete + restore round-trip
  (`cancel_order`/`restore_order`, the tombstone reads back in shape then vanishes from a live list).
  `SeqIdGen` for deterministic demo ids (production uses `UuidGen` behind the `serve` feature).
- **Gate.** The example builds + its scenario runs **green** via `cargo run`; the workspace stays green
  with `examples/` excluded (`cargo test --workspace --all-features`, `fmt --check`, `clippy` all clean,
  example crate included). **Open (B2):** the MariaDB + Postgres slices — the same scenario against those
  servers via Docker.

## D61 — Example projects: the MariaDB + Postgres quickstart slices (Track B / DoD #2 complete)
The MariaDB + Postgres slices of B2, mirroring the SQLite quickstart (D60) so **all three target
dialects have a copyable, runnable reference** — one worked project per DB (DoD #2). `examples/mariadb-
quickstart` and `examples/postgres-quickstart` are standalone crates, workspace-excluded like the SQLite
one, each with its own gitignored `target/` + committed `Cargo.lock`.

- **Deliberately near-identical to the SQLite slice.** Same `schema/` (`@scope Tenant`, nested to-one
  `placed_by { name, email }` shape, keyset `page`, soft-delete), same `build.rs` codegen pattern
  (front end as a library → typed client + DDL into `OUT_DIR`, no checked-in generated code — only the
  DDL `Dialect` differs), same ~20-line `InProcess` `Transport` bridge, same scenario + assertions
  (create → read-your-writes → get → list/scope → keyset paginate → soft-delete/restore). *The point of
  the reference set:* the engine is driver-agnostic, so moving an app between dialects swaps only the
  driver + manifest `dialect`, nothing else. Three things legitimately differ, all forced by the server:
  the **driver** (a pooled `ShardRouter`/`MariaDb` resp. `PgRouter`/`PostgresDb` over a live
  `DATABASE_URL`, checked out as the engine's `Db`, not an in-memory `SqliteDb`), the **id generator**
  (`UuidGen`, the production one — MariaDB/Postgres native `uuid` id columns reject non-uuid ids, so
  `SeqIdGen`'s `id-N` won't do), and the **fixture ids** (real v4 UUIDs). Each example resets its three
  tables on startup (`DROP TABLE IF EXISTS` → generated DDL → seed) so it is re-runnable against a
  persistent server; MariaDB runs the setup script statement-by-statement over the `Db` seam, Postgres
  via a one-shot `pg_connect(...).batch_execute(...)`. `DATABASE_URL` env (documented default matching a
  throwaway Docker server) supplies the connection.
- **A real Postgres driver bug the example path surfaced + fixed (like D59).** rust-postgres returns
  result columns in **binary** format (format code 1), but `from_pg` decoded every non-numeric column as
  raw UTF-8 text. Coincidentally fine for `text`/`int` (which no prior live test exceeded), but a `uuid`
  arrived as 16 raw bytes (hex-encoded to a *hyphen-less* string — accidentally re-parseable, so it hid)
  and a `timestamptz` as an i64 of microseconds → hex garbage → `invalid input syntax for type timestamp
  with time zone` when a keyset cursor fed that value back on page 2. Fixed in `postgres.rs`: `from_pg`
  now decodes the binary layouts explicitly — `uuid` (16 bytes → canonical `8-4-4-4-12`), `timestamptz`/
  `timestamp` (µs since 2000-01-01 → ISO `YYYY-MM-DD HH:MM:SS[.ffffff]+00`), `date` (days → `YYYY-MM-DD`,
  via a dependency-free Hinnant `civil_from_days`), `jsonb` (strip the version byte) — so each round-trips
  as the same string a text read/literal would and re-binds exactly (keyset equality holds). Pure
  decoders unit-tested; a live regression test (`uuid_and_timestamp_columns_round_trip_and_keyset`)
  projects a uuid + `timestamptz` and keyset-pages on the timestamp against a real Postgres.
- **Gate.** Both examples **build and run green** (`cargo run`) against live `mariadb:11.4` + `postgres:16`
  (Docker/OrbStack), twice each (proving the reset). `cargo test --workspace --all-features` green
  (11/11 Postgres live tests incl. the new one), `fmt --check` + `clippy` clean across the workspace and
  both new example crates, `examples/` still workspace-excluded. **Track B / DoD #2 complete** — a worked,
  runnable project per target DB (SQLite D60, MariaDB + Postgres D61).

## D62 — `based gen client` emits the in-process embedded bridge (`client::embedded(&engine)`)
The typed client is generic over a `Transport` trait it *defines itself*, so — orphan rule — a
library-side `impl Transport for Engine` in based-runtime is forbidden (the trait is owned downstream,
in the generated module). The prior consequence: every in-process embedder hand-copied the same ~20-line
`InProcess` bridge (`tests/embed.rs`, all three quickstarts). Since the quickstarts are a user's first
look at a library whose promise is *effortless* Rust↔DB access, that plumbing is exactly the wrong first
impression. Fix at the source: **`based gen client` emits the bridge.**

- **Emitted surface.** When enabled, the client module also carries an `Embedded<'a>` transport wrapping
  a `&'a based_runtime::Engine`, its `impl Transport` (serialize typed input + `$ctx` to JSON — a
  non-object ctx → `{}`; `engine.call(route, args, ctx)`; `200` → decode body into `O`, non-`200` →
  `ClientError` from `error.message`, byte-for-byte what the hand bridge did), and a free constructor
  `pub fn embedded(engine: &Engine) -> Client<Embedded<'_>>`. Ergonomics: `let api = client::embedded(&engine);`
  then `api.place_order(input, ctx)?` — **zero** bridge code. `based_runtime::Engine` is named **by path
  in the emitted text**; based-codegen keeps *no* based-runtime dep (that would be circular) — the
  consuming crate is what depends on based-runtime.
- **Gating (why opt-in, not a cfg feature).** A pure-wire/HTTP client need not depend on based-runtime,
  so the `based_runtime::Engine` reference must not be forced on it. Chose an **emit flag on the library
  entrypoint** (`ClientOptions { embedded: bool }`, threaded through a new `client_with(schema, decls,
  target, opts)`; `client(…)` stays the default-off wire entry) over a `#[cfg(feature = "…")]` guard:
  a cfg feature would need the *consuming* crate to declare and enable a matching feature (friction +
  confusion for `include!`d generated code), whereas an emit flag makes the module text either contain
  the bridge or not — the wire CLI path (`based gen client`) leaves it off untouched, and an embedding
  `build.rs` opts in with one field. Keeps the wire client unaffected and the embed path a one-liner.
- **`#![allow(dead_code)]` wart fixed at the source.** The preamble no longer emits an inner
  `#![allow(dead_code)]` (which `include!` rejects — inner attributes must annotate an enclosing
  file/module, forcing every embedder to do `.replace("#![allow(dead_code)]\n", "")`). Consumers now
  apply an outer `#[allow(dead_code)] mod client { … }` (the standard pattern for generated code); the
  quickstarts already did, so their now-vestigial `.replace(…)` is a harmless no-op until the next
  iteration removes it. Existing `based gen client` consumers keep working.
- **Seam proven, plumbing deleted.** `tests/embed.rs` now consumes the emitted `client::embedded(&engine)`
  instead of its hand-written `InProcess` impl (the committed verbatim `mod client` was regenerated with
  `embedded: true`), validating the bridge end-to-end and removing duplicated code. A codegen unit test
  asserts the bridge is present with the flag and absent (no `based_runtime` reference) without it, plus
  that no inner attribute is emitted.
- **Gate.** `cargo test --workspace --all-features` green (incl. the reworked embed test + the new codegen
  tests), `fmt --check` + `clippy --workspace --all-features` clean, all three `examples/` still `cargo
  build` (their own bridge stands for now — the quickstart DX rebuild onto `client::embedded` is the next
  iteration). **Unblocks the quickstart rebuild.**

## D63 — Quickstart DX rebuild: the three quickstarts read as copyable references, not integration tests
The three quickstarts are a user's first look at a library whose whole promise is *effortless* Rust↔DB
access, but the D60/D61 versions read like internal integration tests: a `build.rs` regenerated the
client on every build, schema setup was a raw-DDL `include_str!` + `DROP TABLE` string const, seeding
was a raw `INSERT` const, and each hand-copied the ~20-line `InProcess` `Transport` bridge D62 had just
made unnecessary. Rebuilt all three (SQLite/MariaDB/Postgres) into genuinely copyable "run it as-is"
references. The target shape a user follows: **(1)** copy the dir, set `DATABASE_URL` in `.env`; **(2)**
`based migrate apply` → tables; **(3)** `based gen client -o src/client.rs --embedded` → typed client;
**(4)** `cargo run` → the end-to-end scenario, green-or-exit-nonzero. Steps 2–3 ship pre-generated +
checked in (reviewable artifacts, migrations.md), so the run is just steps 1 + 4.

- **Schema setup = `based migrate apply` (the real convention, E4), never raw DDL.** Each example
  carries a checked-in `migrations/0001_init/` (generated by `based migrate gen`); the README's step 2
  applies it live. `main.rs` issues **no DDL** — it loads the already-migrated DB. This also gave the
  **first live Postgres `migrate apply`** (D42 had only proven MariaDB + SQLite live); it worked
  unchanged — no Postgres migrate bug surfaced.
- **The `Transport` bridge is deleted; `client::embedded(&engine)` (D62) is the whole of the wiring.**
  `main.rs` is `let api = client::embedded(&engine);` — zero bridge. `src/client.rs` is the verbatim
  `based gen client --embedded` output, checked in as a normal `#[allow(dead_code)] mod client;` file
  (no `build.rs`, no `include!`, no `.replace(…)` surgery).
- **New CLI surface: `based gen client --embedded`.** The CLI exposed only the wire client
  (`client(…)`); added an `--embedded` flag wired to `client_with(…, ClientOptions { embedded: true })`
  so the D62 emit path is reachable from the command line, symmetric with the `based migrate` CLI
  convention the examples now use. Off by default (a pure-wire consumer needs no based-runtime dep).
- **Zero raw SQL in the run.** No `INSERT`/`DROP TABLE`/`SCHEMA_SQL`/`SEED_SQL` consts. Seeding is the
  client's own `create_org`/`create_user` mutations (added to the schema — the conventional API, and
  the engine mints + returns each id, so the scenario captures real ids instead of hard-coding
  `org-acme`). The scenario is unchanged in what it proves (create → read-your-writes in the declared
  `OrderCard` shape → get → list/`Tenant`-scope → keyset paginate → soft-delete/restore).
- **`.env` + dotenvy, no hard-coded URL.** Each ships a committed `.env` with a `DATABASE_URL`
  (SQLite = a file path; MariaDB/Postgres = the throwaway-Docker URL); `main.rs` loads it with dotenvy,
  and the README passes it to `based migrate apply --database-url "$DATABASE_URL"` after `source .env`.
- **Re-runnability = fresh DB per run (no raw reset).** The `slug`/`email` uniques reject a second seed,
  so the README documents starting clean: delete the SQLite file, or recreate the throwaway container.
  No idempotency-key seeding was needed (a persistent-server story could use it, but fresh-DB is the
  cleaner default the task blessed).
- **Structure.** `based.toml` moved to each example root (`root = "schema"`) so `migrations/` sits at
  the project root (migrations.md) and `based <cmd>` runs from the dir with the default `.` root. The
  three stay structurally identical, differing only where the server forces it: driver, id generator
  (`SeqIdGen` for SQLite, `UuidGen` for the uuid-id servers), and `DATABASE_URL`.
- **Gate.** All three build **and run green** following their README steps against fresh databases —
  SQLite locally, MariaDB `11.4` + Postgres `16` via Docker/OrbStack (live `migrate apply` + `cargo
  run`, output pasted in the iteration). `cargo test --workspace --all-features` green, `fmt --check` +
  `clippy` clean across the workspace and all three example crates, `examples/` still workspace-excluded.

## D64 — Track D2 CI: portable "keep-proven" targets + a thin GitHub Actions example

DoD #4's CI half (keep the real-DB suites + example scenarios + migration-apply + the extension
build from rotting). Framing (user, 2026-07-08): **GitHub Actions is an example, not the
substance** — the runnable commands live in portable `make` targets any CI (or a laptop) invokes;
the workflow is a thin wrapper that only provisions infra and calls them. D1 (the `based serve`
Dockerfile/image) stays open — a separate iteration.

- **Env-URL override is the crux (the "migrations/tests in CI" ergonomic).** The live suites used
  to always self-spin a container via `docker-tests`. Now `support/docker_{mariadb,postgres}.rs`
  first check `TEST_MARIADB_URL` / `TEST_POSTGRES_URL`: when set, the harness connects to *that*
  server (a CI service container) after the same in-process readiness-wait, and its `Drop` leaves
  the external server alone; when unset, the existing self-spun-container path runs unchanged
  (still skips cleanly with no daemon). `MariaDbContainer`/`PostgresContainer` became a two-variant
  enum (`Spun { id, port }` / `External { url }`); the readiness poll (`wait_ready`) is now a free
  fn over a URL, shared by both. Proven: the suites logged `using external TEST_…` and connected to
  the provided servers (11 Postgres, 10 MariaDB, + 2 live `migrate apply`), and `--all-features`
  still self-spins them locally.
- **External DBs persist ⇒ reset per test.** A self-spun container is fresh; a shared CI server is
  not, and tests would collide on `CREATE TABLE`/seed. Each suite helper now resets before creating:
  MariaDB drops its schema's tables + `_based_migrations` with `FOREIGN_KEY_CHECKS=0` (one
  connection, session-scoped); Postgres does `DROP SCHEMA public CASCADE; CREATE SCHEMA public`;
  the MariaDB apply suite drops `widget` + the ledger. Idempotent + re-runnable (verified by a
  second green run). The live targets pass `--test-threads=1` so the resets stay serial.
- **Portable readiness-wait for the CLI path.** The live suites wait in-process; the *examples* call
  `based migrate apply` as an external command, so `ci/wait-for-db.sh <url> [timeout]` blocks on a
  bash-builtin `/dev/tcp` connect (no psql/mysql client) before apply — a sqlite file path is a
  no-op. GH service `--health-cmd`s are belt-and-suspenders on top.
- **Portable targets (`Makefile`).** `ci-workspace` (fmt + clippy `-D warnings` + test),
  `ci-extension` (`npm ci` + compile + `vsce package`), `ci-live-mariadb` / `ci-live-postgres`
  (the live suites against `$(MARIADB_URL)` / `$(POSTGRES_URL)` via `TEST_*_URL`), `ci-examples`
  (build `based`, then apply + `cargo run` each quickstart against provided URLs), plus
  `dev-db-up`/`dev-db-down` throwaway `mariadb:11.4` + `postgres:16` for local runs. URLs are
  overridable vars defaulting to the throwaway containers.
- **Thin GH Actions example (`.github/workflows/ci.yml`).** Five jobs (`workspace`, `extension`,
  `live-mariadb`, `live-postgres`, `examples`); the DB jobs declare `services:` `mariadb:11.4` +
  `postgres:16` with health checks and pass the service URL to the matching `make` target. No
  bespoke marketplace choreography — checkout + toolchain + `make`. Valid YAML, sane structure.
- **Gate.** `cargo test --workspace --all-features` green (docker suites self-spun in the fallback
  path); `fmt --check` + `clippy -D warnings` clean; all portable targets ran green locally against
  live `mariadb:11.4` + `postgres:16` — both live suites + `migrate apply` against provided URLs,
  all three example scenarios, and the extension `.vsix` packaged.

## D65 — Live-DB hardening: statement timeouts, deadlock-retry, pool-exhaustion→fast-503

Track A4 (DoD #1 hardening). A dependable engine must not *hang* under adverse live conditions —
a runaway query, a lock conflict, or a saturated pool. Three per-dialect mechanisms, all classified
through one runtime seam and proven against live `mariadb:11.4` + `postgres:16` (not just designed).

- **One classification seam: `DbError.kind: DbErrorKind`.** `DbError` grew a `kind`
  (`Other` | `Deadlock` | `PoolExhausted`, `#[default] Other`). Every `DbError` is still the wire's
  `503`; the kind only changes *engine* behaviour: `Deadlock` is retried, the rest fail through. Each
  driver maps its own server error codes (the runtime stays dialect-neutral): MariaDB 1213/1205 →
  `Deadlock`, Postgres `40P01`/`40001` → `Deadlock` (a `57014` statement-timeout cancel stays `Other`
  — retrying would just time out again), SQLite `SQLITE_BUSY`/`SQLITE_LOCKED` → `Deadlock`; a checkout
  wait that times out → `PoolExhausted`. `DbError::new` keeps the `Other` default so no call site broke.

- **Statement timeouts (server-side, per-dialect).** A query/mutation past a ceiling is aborted *by
  the server*, surfacing as a `503` rather than holding a connection. MariaDB sets session
  `max_statement_time` (seconds; unlike MySQL's SELECT-only `max_execution_time` it caps *every*
  statement) via the pool's per-connection `init`; Postgres sets `statement_timeout` (ms) as a startup
  `options` param so it applies to every statement on a pooled connection; SQLite sets a bounded
  `busy_timeout` on open (the realistic under-contention concern for a file DB). Configured on
  `PoolConfig.statement_timeout` (`Duration::ZERO` disables); default 30s — a runaway-query backstop,
  not a tight SLA a deployment tightens.

- **Deadlock-retry (bounded, with backoff).** A deadlock/serialization abort rolled the transaction
  back server-side, so re-running it usually succeeds once the winner commits. `run::apply` now wraps
  the transaction (`apply_once`) in a loop: on a `Deadlock`-kind error it re-runs the *whole* tx up to
  `TX_RETRY_LIMIT` (5) times with a short exponential+jittered backoff (≤100ms; jitter so two
  deadlocked txns don't collide in lockstep), then surfaces the `503`. Bounded so a pathological hot
  row fails fast, not forever. Retries only the write path (reads that fail are the client's to retry).

- **Pool-exhaustion → fast 503, never a hang.** A saturated pool must fail fast so a worker thread is
  never tied up unboundedly. `PoolConfig.checkout_timeout` (default 5s) bounds the wait: the MariaDB
  router uses `try_get_conn(timeout)` (the `mysql` pool otherwise blocks indefinitely), the Postgres
  router sets r2d2's `connection_timeout`; both map a checkout timeout to `PoolExhausted` → the HTTP
  edge's existing checkout-failure→503 path (D21). `PoolConfig` gained the two `Duration` fields
  (stays `Copy`); `based serve` fills pool sizing from its flags and keeps the timeout defaults.

- **Verified live (the DoD bar).** New live tests in `tests/{mariadb,postgres}_integration.rs`
  (feature `docker-tests`, against real containers): (1) `SELECT SLEEP(5)`/`pg_sleep(5)` under a 500ms
  timeout returns an error in <3s, not after the full sleep; (2) two threads lock two rows in opposite
  order behind a `Barrier` (deterministic crossed-lock) → exactly one side aborts with a `Deadlock`
  kind, the other commits; (3) a pool of one with a held connection fails the next checkout fast as
  `PoolExhausted`, not a hang. The retry *loop* (retries-then-commit + gives-up-after-bound) is
  unit-proven with a fake `Db` in `tests/mutation.rs` (deterministic, no reliance on racing a server
  to hit the exact retry path), and SQLite `SQLITE_BUSY`→`Deadlock` classification is unit-proven
  against a real two-connection file DB in `sqlite.rs`.

- **Gate.** `cargo test --workspace --all-features` green *with Docker up* — the three MariaDB + three
  Postgres hardening tests booted real `mariadb:11.4`/`postgres:16` containers and passed (MariaDB
  suite 13, Postgres 14); `fmt --check` + `clippy --all-features` clean.

## D66 — `based serve` container image (Track D1, DoD #4 deploy half)

Packages `based serve` (the D21 listener with D26 health/readiness/drain) as a deployable image.
The artifact and two enabling seams:

- **`based serve` is now dialect-aware.** It previously hardcoded the MariaDB `ShardRouter`, so it
  only ever served MySQL/MariaDB regardless of the manifest dialect (a footgun for a "deployable" —
  a Postgres project would silently point a MySQL driver at Postgres). `cmd_serve` now branches on the
  manifest `Dialect` and builds the matching backend — `ShardRouter` (MariaDB), `PgRouter` (Postgres),
  or a single-file `SqliteBackend` — each handed to a small generic `run_listener` (the listener is
  already `Backend`-generic, D21). One image serves whatever `based.toml` targets.
- **Env config seams** (a container configures by env, not flags). `--listen` gains `env = "BASED_LISTEN"`
  (flag > env > `127.0.0.1:8080` default; the image sets `0.0.0.0:8080` so the port is reachable), and
  the db-url resolver additionally honors the ubiquitous `DATABASE_URL` after `BASED_DATABASE_URL` — the
  convention the quickstarts + most hosts use. `cmd_serve` now shares the migrate path's `shard_urls`.
- **The image** (`docker/Dockerfile`). Multi-stage: a `rust:1-bookworm` builder compiles the release
  binary (BuildKit cache mounts keep an unchanged dep set from recompiling — the cache-friendly split
  without a stub-crate dance), a `debian:bookworm-slim` runtime carries just the binary + entrypoint
  (~122 MB, unprivileged user). Carries **no schema** — the project (`based.toml` + `**/*.bsl`
  [+ `migrations/`]) mounts at `/app`; everything else is env. `entrypoint.sh` optionally runs
  `based migrate apply` first (`BASED_MIGRATE_ON_START=1`, opt-in — safe by default) then `based serve`.
  `HEALTHCHECK` runs `docker-healthcheck.sh` (a script, so the port is derived in the shell at runtime,
  not Docker's build parser) probing `/healthz` — never touches the DB, so a DB blip drains via `/readyz`
  instead of restarting the box.
- **CI keep-proven** (matching D64's shape): a portable `make ci-image` builds the image then
  `ci/smoke-image.sh` boots it against the bundled-SQLite quickstart (no external service) and asserts
  `/healthz` + `/readyz` answer 200; a thin `image:` job in `ci.yml` calls it.

**Verified live.** Built the image, ran it on a Docker network against an ephemeral `postgres:16`
(`BASED_MIGRATE_ON_START=1` applied `0001_init` on boot, served the Postgres dialect): Docker
`HEALTHCHECK` went `healthy`, `/healthz` + `/readyz` = 200, then the full scoped flow through the
container — `create_org`/`create_user` (public) → `place_order` (scoped write, nested `placed_by` shape)
→ `my_orders` (scoped read returns it) → a different tenant's `$ctx` sees `[]` (isolation). `SIGTERM`
drained ("shutdown signal received, draining…") and exited 0. `make ci-image` also green (SQLite smoke).
Gate: `fmt --check` + `clippy --all-features -D warnings` + `cargo test --workspace --all-features` clean.

## D67 — `@was` renames + offline drift diagnostic + `raw(dialect)` up step (Track E5, DoD #5 fully met)
The last migration gap: the `@was` rename directive end-to-end, the offline schema-vs-migrations drift
diagnostic, and the `raw(dialect)` escape step. Settles migrations.md's open E5 items; DoD #5 goes
Core-met → **fully met** ("a `.bsl` change produces a reviewable, editable migration you can safely
apply to an existing DB" — now including renames that preserve data).

- **`@was` is parsed both forms** (grammar already anticipated it): field-level `<field>: <ty>
  @was("old_col")` sits in the field modifier position (parsed in `field_after_colon`, stored on
  `Field.was: Option<Ident>` carrying the old name + literal span); model-level `@was("old_table")` is a
  generic decorator (read in `model::model_was`). Sema threads them onto `RModel.was` / `RMember.was`.
- **Sema validation** (`based-sema::model::validate_was`, D-codes in `ir::code`): `@was` names a
  *previous* name that lives only in the migration snapshot, so sema can't confirm it existed (the diff
  does) — it catches the two locally-decidable mistakes: **`E0190`** a no-op self-rename (old == current
  name), **`E0191`** the old name is still a *live* column/table (then it can't be the rename's source).
  `"was"` joins `KNOWN_DECORATORS` (no more `W0101`).
- **Renames are snapshot-authoritative, never auto-guessed** (principle 2). `Snapshot` gains a
  `renames: Vec<Rename>` (Table/Column) built from the schema's `@was` in `from_schema`, **persisted in
  `schema.snap`** (`rename table <old> -> <new>` / `rename column <table>.<old> -> <new>` lines,
  round-tripping through `parse`) so `apply`/`render`/`verify` re-derive the rename from the stored
  snapshots with no DB. `diff_snapshots` reads `now.renames`: a valid rename (old in `prev`, new in
  `now`, new absent from `prev`) emits one `Step::RenameTable`/`RenameColumn` instead of a drop+add — a
  **spent** `@was` (old already gone) is inert (no step), so leaving it in the `.bsl` is harmless.
  `verify`'s "uncaptured changes" check switched from raw snapshot equality to `diff_snapshots(...).
  is_empty()` so a lingering/spent `@was` never reads as false drift.
- **Per-dialect rename SQL** (`migrate::sql`): `ALTER TABLE … RENAME TO …` (table) and `ALTER TABLE …
  RENAME COLUMN … TO …` (column) — a safe in-place ALTER on all three (Postgres always; MariaDB ≥10.5.2
  / SQLite ≥3.25 for `RENAME COLUMN`; `RENAME TO` universal), so data survives. Non-destructive.
- **`raw(dialect)` escape step** (`Step::Raw { dialect, sql }`). The diff never *generates* it (opaque
  SQL the neutral vocabulary can't model); it is **authored into `up.mig`** and recovered by
  `migrate::parse_raw_steps` (dialect between `raw(` `)`, SQL between the first/last backtick so an inner
  MariaDB identifier-backtick survives). `apply`/`render` layer the matching-dialect raw steps after the
  snapshot-derived structural steps; a non-matching dialect is a no-op (its per-dialect twin carries the
  change). Not offline-verifiable — `verify` compares the stored `up.mig` with its `raw(` lines stripped
  (`has_raw_step`/`strip_raw_steps`) and reports the migration **`partial`** (principle 6, never silent).
- **Offline drift diagnostic** (LSP, `based-lsp::compile`; `based migrate verify` is the CLI twin, D42).
  `compile_manifest` passes the project root to `compile_paths`, which — only when the project has
  captured migrations — diffs the latest `schema.snap` against the current schema (`migrate::drift`) and
  publishes **`W0108`** "N uncaptured schema changes — run `based migrate gen`", anchored per changed
  model (`Step::table_name`/`describe`). A **spent `@was`** (rename already captured) surfaces as
  **`W0107`** at the `@was` literal, "rename already captured — remove it".

**Verified live** (Docker: `postgres:16` + `mariadb:11.4`). Postgres: `0001_init` created `product` with
`name`/`upc`, seeded a row (`Widget`/`012345678905`); renamed `upc → barcode` via `@was`, `migrate gen`
emitted the one `rename column` step, `render` showed `ALTER TABLE "product" RENAME COLUMN "upc" TO
"barcode"`, `apply` ran it, and `SELECT name, barcode` returned the *same* row (data survived a real
RENAME, not drop+recreate). Removing the spent `@was` kept `verify` clean + `gen` a no-op. A hand-authored
`raw(postgres)` backfill (`UPDATE … SET note = 'seeded'`) added-column-then-backfilled live and `verify`
reported that migration `partial`. MariaDB: the same `@was` cycle (twice, on valid-uuid data) applied
`ALTER TABLE `product` RENAME COLUMN …` and the row survived. Gate: `cargo test --workspace --all-features`
+ `fmt --check` + `clippy --all-features` clean; new sema cases (E0190/E0191 ± clean), codegen diff/render
unit tests (rename one-step, spent-inert, snap round-trip, raw per-dialect), and LSP drift/spent tests.

## D68 — Folding + selection ranges (Track C4 feature-parity complete)
The two remaining C4 gaps in the LSP capability audit — `textDocument/foldingRange` and
`textDocument/selectionRange`. Both advertised at `initialize` (`folding_range_provider` /
`selection_range_provider` = `Simple(true)`) and served from the parsed decl spans already in the
snapshot (`based-lsp::compile`), same routing (nearest-manifest snapshot) as the other position
requests. The extension is a thin client — `vscode-languageclient` negotiates a newly advertised
capability automatically, so no `editors/vscode` source change was needed (the TS still compiles).

- **Folding** (`Snapshot::folding_ranges`): one `Region` fold per top-level declaration whose body
  spans more than one line — model / shape / scope / query / mutation / filter. The range runs from
  the body's opening `{` (found within the decl's own span, so the `Name {` header stays visible when
  collapsed) to the span's closing byte; a brace-less multi-line decl folds from its first line. No
  file-wide brace scan or re-parse — extents come off the `decl_span` the parser already stamped.
- **Selection ranges** (`Snapshot::selection_range`): the expand/shrink-selection hierarchy at a
  position, built by collecting every AST span covering the offset (a model field's `name` → `ty` →
  whole-field, its enclosing declaration) plus the identifier token under the cursor and the whole
  file, then keeping a strictly-nesting chain (sorted by width, each level contained in its parent)
  linked innermost-out. The handler returns one `SelectionRange` per requested position (LSP requires
  index alignment); a position on nothing resolvable falls back to a bare cursor range.
- **Shared token scan:** the identifier-extent walk `prepareRename` used is factored into
  `word_extent`, reused as the selection-range token level (one ASCII-safe byte scan, not duplicated).
- **Code actions — declined (out of scope), documented.** The audit flagged `codeAction` (e.g. `W0103`
  → add `@index`) as "borderline, only if cheap." It isn't: `W0103` is anchored on the *query*'s clause
  span, often in a different file than the model that needs the index, and the diagnostic carries no
  target-model span or column tuple — a correct quick-fix would need new plumbing threading the target
  model + suggested columns from the lint through the diagnostic into the LSP. Not cheap; left as a
  documented gap (README row stays "out of scope"), to revisit when a `based fmt` / edit-producing
  surface exists.
- **Static editing config verified:** `editors/vscode/language-configuration.json` already covers
  bracket matching, auto-closing + surrounding pairs (`{}`/`[]`/`()`/`""`/` `` `), and `#` line-comment
  toggling — no change needed.

**Track C4 (VS Code / LSP feature-parity) is complete**, so DoD #3's "feature-parity fill-in" closes and
every DoD item is met. Gate: `cargo test --workspace --all-features` + `fmt --check` +
`clippy --workspace --all-features` clean; `cargo build -p based-lsp` + `npm run compile` clean; two new
LSP unit tests over the commerce fixture (folding: model/shape fold, single-line scope doesn't; selection:
`total` token → field → `Order` model → file, strictly nesting).

## D69 — Comment-hygiene sweep (Track F1, last roadmap item)
The one-time cleanup the Conventions rule promised: sweep every `crates/**/*.rs` for build-time / WIP /
narration comments and rewrite them into brief what+why matching surrounding density. **Standing rule
enforced** (CLAUDE.md Conventions): source is *finished* source, not a scratch pad — no "here's what I'm
building" / "now we…" running commentary, no historical "used to be X" storytelling; inline TODOs live in
PLAN.md / the roadmap `.md`s, not source, unless genuinely blocking.

- **Finding: the tree was already clean.** `sqlite.rs` (PLAN's flagged "known offender") read as finished
  — D65's live-hardening rewrite had already given it clean what+why comments. No live `TODO`/`FIXME`/
  `XXX`/`HACK` markers remained anywhere in source. Prior iterations' adherence to the Conventions rule
  (and the `source-comment-style` memory) had kept narration out incrementally.
- **Tightened (comment-only, zero logic change):** three residual bits — `based-codegen/src/migrate.rs`
  `schema.snap` grammar header ("(finalizing migrations.md's TODO)" → dropped), `based-codegen/src/client.rs`
  `ClientTarget` doc ("first and only for now" → "the only target") and the embedded-bridge doc ("the bridge
  every embedder used to hand-copy" → "the bridge an embedder would otherwise hand-copy", present-tense).
- **Left as-is (legitimate, not narration):** the `idempotency.rs`/`http.rs`/`lib.rs` "deferred to the
  live-DB slice / multi-instance store" notes are deliberate scope documentation of a known limitation whose
  deferred item is already tracked in PLAN's Deferred list — architecture docs, not a WIP TODO.

**This is the last open roadmap item**, so with it done every roadmap track (features + hygiene) is complete
and the project is fully done per the Definition of Done — only deferred nice-to-haves remain. Comment-only,
so the gate is `cargo test --workspace --all-features` + `fmt --check` + `clippy --workspace --all-features`,
all clean.

## D70 — Typed ids in the generated client (per-entity phantom `Id<E>` newtypes)
Post-DoD backlog item **H1**. The client lowered every id/relation param to a bare `Uuid = String` alias,
discarding the schema type the front end already knew: `place_order(buyer: Id)` where `buyer -> User` emitted
`buyer: String`, identical to `org`, `id`, and every `create` result's `.id` — so any id transposition (an
`Org` id where a `User` id belongs, or a literal string) type-checks and fails only at runtime (FK violation /
empty result), undercutting the DB-first-typed-access thesis at the one place a human/LLM writes a call by hand.

**Decision: emit per-entity phantom-typed newtypes.** `based gen client` now emits `pub struct Id<E>` — a
`#[serde(transparent, bound = "")]` newtype over the raw id string, plus a `pub mod entity { pub enum <Model> {} … }`
of zero-value phantom tags — so a model's own `id`, a relation param/FK, and a `$ctx` relation are all typed
`Id<entity::M>`, distinct per entity. `Id<User>` and `Id<Org>` are different types → the transposition is a
compile error, not a runtime FK failure. (A single undifferentiated `Id` would still let org/user ids swap, so
the phantom tag is load-bearing.)

- **Wire unchanged.** `transparent` means the JSON/OpenAPI surface is still the underlying string
  (`{ type: string, format: uuid }`), so `based-codegen::openapi` needed **no** change — its `uuid_schema()` is
  already coherent. No custom serde. The skipped `PhantomData<fn() -> E>` field keeps `Id<E>` `Send`/`Sync`/
  covariant and imposes no bound on the tag.
- **Safe edges only; the unsafe edge is greppable.** A `create_*` result already *is* the typed id, so
  create→use chains need no conversion. There is deliberately **no** blanket `From<String>` (that reopens the
  hole); a raw string becomes a typed id only through the explicit `Id::from_raw(s)` — mirroring how `unscoped(…)`
  makes the unsafe escape visible (principle 1/6). The trait impls (`Clone`/`Eq`/`Ord`/`Hash`/`Display`/`Debug`)
  are hand-written so the marker `E` carries no bounds (a derive would demand `E: Clone`, … of a pure tag).
- **"Stop discarding what the front end knows," not new analysis.** Param entities are resolved from the same
  edges sema type-checks: a query param via its binding (`-> edge` / `op col` / same-name column) against the
  target model; a mutation param by walking the write body (assigned to a Forward FK / `id`, or compared in a
  `where`). A bare `Id` annotation whose body use resolves to `placed_by -> User` becomes `Id<entity::User>`.
- **Per-target idiom** (future): Rust newtype now; a TS branded type / Go named type when those targets land.

Touched `based-codegen::client` (the codegen), its `tests/client.rs` (typed-id assertions + a new phantom-newtype
test), the regenerated verbatim client in `based-runtime/tests/embed.rs` (its test bodies now build ids via
`Id::from_raw`, the one boundary that has raw literals), and all three `examples/*/src/{client.rs,main.rs}`
(regenerated client; helper signatures take typed ids, no `.to_string()`). Gate: `cargo test --workspace
--all-features` + `fmt --check` + `clippy` all clean; all three quickstarts ran green live via `cargo run`
(SQLite bundled; Postgres `postgres:16` + MariaDB `mariadb:11.4` over Docker).

## D71 — Production-grade errors on the user-facing path (client `ClientError` + runtime errors)

**Context (H7, user-raised).** Audit of the error surface a *library user* meets found the user-facing
error type was not production-grade. The generated client's `ClientError` was `pub struct ClientError(pub
String)` — an opaque string, no `Display`, no `std::error::Error`, no `source()`, no machine code or HTTP
status, and the embedded bridge *threw away* the wire envelope's `code` + `status`, flattening every server
error to a message string a caller could only string-match. The runtime errors it surfaces (`PlanError`,
`DbError`, `RunError`) were `Debug`-only: no `Display`, no `Error`, no `?`-chaining, and the stable wire
codes lived only as string literals inside `serve::plan_error_response` (drift risk, principle 4).

**Decision.** Make the whole user-facing error path structurally sound and ergonomic.

- **Generated client `ClientError` is a real error.** Now a struct carrying a `ClientErrorKind`
  (`Transport` / `Decode` / `Api { status, code }`), a human `message`, and an optional `source`
  (an `Arc<dyn Error + Send + Sync>` so the type stays `Clone` while `source()` hands back a live
  `&dyn Error`). It implements `Display` (kind-prefixed clear message) and `std::error::Error` (chains
  with `?`). Accessors let a caller branch without matching message text: `kind()`, `message()`,
  `code()` (the server's `error.code` for an api failure, else `"transport"`/`"decode"`), `status()`.
  Constructors `transport(e)` / `decode(e)` / `api(status, code, message)` for any transport (the
  embedded bridge and a hand-written HTTP transport alike). The embedded bridge rebuilds the api error
  from the `{ error: { code, message } }` envelope, **preserving status + code + message** instead of
  dropping them.
- **Runtime errors implement `Error` + `Display` with a stable `code()`.** `PlanError`, `DbError`, and
  `RunError` now implement `std::fmt::Display` and `std::error::Error` (`RunError::source()` chains to
  the inner `PlanError`/`DbError`). `PlanError::code()` / `DbError::code()` / `RunError::code()` are the
  **single source of truth** for the machine codes; `serve::dispatch` consumes `e.code()` + `e.to_string()`
  and maps only the HTTP *status* (the wire concern), so the codes can no longer drift from a duplicated
  literal. `DbError::code()` distinguishes `deadlock` / `pool_exhausted` from a generic `database_error`
  (all still `503`) — more precise than the previous always-`database_error`. `Family::label()` centralizes
  the family name used in a boundary message.
- **Messages are userland.** Clear, actionable, no jargon, no decision-refs. `$ctx.<field>` (real DSL
  syntax) replaces the previous `${ctx}.<field>` in boundary messages.

**Wire contract:** unchanged shape (`{ error: { code, message } }`, statuses); OpenAPI describes the
envelope generically (no code enum), so it needed no change. The added `deadlock`/`pool_exhausted` codes
are strictly more informative on the existing `503`.

**Deferred (recorded as H7 sub-items in PLAN).** CLI `anyhow`-blob errors (`based migrate`/`gen`/`serve`
diagnostics), the HTTP listener edge's own error bodies beyond dispatch, and converting the example
`main.rs` scenario to a `?`-based `Result` flow (it already benefits from the richer `Debug`/`Display`
through `.expect`).

Touched `based-codegen::client` (the `ClientError` type + embedded bridge), `based-runtime::{plan,run,
serve,value}` (traits + `code()` accessors + single-source wire mapping), the verbatim client copy in
`based-runtime/tests/embed.rs`, and all three regenerated `examples/*/src/client.rs`. Gate: `cargo test
--workspace --all-features` + `fmt --check` + `clippy` all clean; all three quickstarts ran **green live**
via `cargo run` (SQLite bundled; MariaDB `mariadb:11.4` + Postgres `postgres:16` over Docker).

## D72 — Production-grade CLI error handling (structured `CliError` + exit-code convention)

**Context (H7 deferred sub-item (i), continuing D71).** D71 fixed the errors a *library user* meets
(the generated client + the runtime types it surfaces). The **CLI** is the other place a user meets
errors directly, and it was not production-grade: `main` returned `anyhow::Result<()>`, so every failure
printed anyhow's `Error: …` blob and exited `1` — a bad manifest, a missing database url, and an
unreachable database were indistinguishable by exit code. Structured runtime errors were re-flattened to
strings (`.map_err(|e| anyhow::anyhow!("{e}"))` on `MigrateError`, `.map_err(|e| anyhow::anyhow!("{}",
e.message))` reaching into `DbError`), throwing away their `Display` + `code()`. The rustc-style
parse/sema diagnostic rendering (`render.rs`), on the other hand, was already good.

**Decision.** Give the CLI one structured top-level error and a real exit-code convention; drop `anyhow`.

- **`CliError` (new `based-cli/src/error.rs`).** A struct carrying a clean, actionable `message`, an exit
  `Kind` (`Usage` vs `Failure`), an optional boxed `source` (`dyn Error + Send + Sync`), and a
  `summary_only` flag. Implements the reporting itself: `report(&self) -> ExitCode` prints a `based: <msg>`
  line plus the `source()` chain (`  caused by: …` per level), then returns the code. Constructors:
  `usage`/`failure` (no cause), `io`/`io_at` (fs failures naming the path), `db` (carries a `DbError`),
  `migrate` (carries a `MigrateError`), `caused_by` (any `Error`), and `summary` (detail already on
  stderr — print the one line only).
- **Exit codes.** `Usage` → **2** (matching clap's own arg-parse exit, so a config/usage mistake is one
  class end to end), `Failure` → **1** (an operational failure the run hit), success → 0. Classified per
  site: no database url / unknown-migration / sqlite-multi-url / a schema that didn't check → `Usage`;
  io / database-unreachable / migration-apply / serve-bind → `Failure`. A `MigrateError::Destructive`
  (needs `--allow-destructive`) is reclassified `Usage` — it's the caller's to fix.
- **Reuse, don't re-stringify (principle 4).** DB and migration failures keep their typed error as the
  `source`, so D71's `MigrateError`/`DbError` `Display` (and `DbError::code()`) read through the chain
  verbatim instead of being flattened into a bare string at the call site.
- **Diagnostics unchanged.** The parse/sema path still renders rustc-style via `render.rs`; those sites
  return a `summary_only` `CliError`, so the user sees the framed diagnostics followed by one terse
  `check failed: N error(s) …` summary (exit 2), never a second anyhow blob on top.

Userland messages carry no jargon / decision-refs. Before → after, a couple of paths: a missing database
url was `Error: no database url: …` (exit 1) → `based: no database url: … set BASED_DATABASE_URL /
DATABASE_URL` (exit **2**); an unreachable database was an `Error:` blob (exit 1) → `based: connecting to
postgres://user@host/db` + `  caused by: <driver message>` (exit 1, url password-redacted).

`anyhow` is dropped from `based-cli`'s dependencies. Touched `based-cli/src/{main.rs,error.rs}` +
`Cargo.toml`. Gate: `cargo test --workspace --all-features` + `fmt --check` + `clippy --workspace
--all-features` all clean; error paths exercised manually (bad manifest, missing file, no db url,
unreachable Postgres, no-migrations render, unwritable output) confirming message + exit code; the SQLite
quickstart still runs green (`based migrate apply` → `cargo run`, exit 0).

**Remaining deferred H7 sub-items:** (ii) HTTP listener edge errors sharing the code registry;
(iii) `based migrate apply` already surfaces the structured `MigrateError` (this slice stopped
re-flattening it) — a dedicated CLI-side type beyond that is unneeded; (iv) example `main.rs` as a
`?`-based error-handling reference matching on `ClientError::kind()`/`code()`.

## D73 — Typed keyset cursor in the generated client (opaque `Cursor` newtype)

**Context (user-raised, mirroring H1/D70).** D70 gave every id a per-entity phantom `Id<E>` so a bare
`Uuid = String` could no longer stand in for a typed id. The keyset pagination **cursor** had the same
untyped hole: the paginated surface carried it as a bare `Option<String>` — the `Page<T>.cursor` field
and the next-page input alike — so it was interchangeable with any other string on the typed surface.

**Decision: emit a single opaque `Cursor` newtype.** `based gen client` now emits `pub struct
Cursor(String)`, a `#[serde(transparent)]` newtype over the underlying cursor string. `Page<T>.cursor`
is `Option<Cursor>` and a keyset page's input `cursor` field is `Option<Cursor>` — so a page result
hands one back and the caller feeds it straight to the next call, a chain that needs no conversion.

- **Single type, not generic per query** (unlike `Id<E>`). A cursor is not entity-typed: it encodes a
  sort-key basis the runtime checksum-validates (`cursor.rs`), and a mismatched cursor is already caught
  at decode (checksum + arity → `bad_cursor`/400). A phantom `Cursor<Q>` would add type-parameter noise
  for a safety property the runtime already enforces, so one opaque `Cursor` is correct and simpler.
- **Wire unchanged.** `transparent` keeps the JSON/OpenAPI surface a bare opaque string
  (`{ type: string }`), so `based-codegen::openapi` needed no change — the cursor stays a `string`/`null`
  in both the envelope and the request body.
- **Opaque + greppable escape.** The traits are the ergonomic subset (`Clone`/`Eq`/`Hash`/`Display`/
  `Debug` — no `Ord`, an opaque cursor has no meaningful order) plus `from_raw`/`as_str`/`into_raw`.
  There is deliberately **no** blanket `From<String>`; a raw string becomes a `Cursor` only through the
  explicit `Cursor::from_raw` — mirroring `Id::from_raw` for the rare cursor that arrives from outside.

Touched `based-codegen::client` (the `Cursor` type + `Page<T>` field + keyset input field), its
`tests/client.rs` (typed-cursor + transparent-newtype assertions), the verbatim client copy in
`based-runtime/tests/embed.rs`, and all three regenerated `examples/*/src/client.rs` (the example
`main.rs` already reads `p.cursor` and passes it back, so it stayed unchanged — the point of the typed
handoff). Gate: `cargo test --workspace --all-features` + `fmt --check` + `clippy` all clean; all three
quickstarts ran **green live** exercising pagination via `cargo run` (SQLite bundled; MariaDB
`mariadb:11.4` + Postgres `postgres:16` over Docker).

## D74 — Editor/comment hygiene: positive framing, `based-codegen` D#-refs, overlong comments (H4/H5)

**Context (three user-raised corrections, one principle: state what a thing *is*, concisely).**

1. **Positive framing over define-by-negation.** A `Page` hover / doc-string read "…rows + an opaque
   cursor, **never a bare array**." Defining a thing by what it *isn't* reads as bizarre. Rule: *say what
   a thing is, never what it isn't.* Rewrote the pagination surface to state only the envelope's contents
   (rows + an opaque cursor; next page = the same call carrying the cursor) in `calling.md`, the emitted
   `Page<T>` doc-comment (already positive in `based-codegen::client`), `openapi::page_schema`, and the
   `openapi`/`client` tests. Swept the source + userland surface for the same define-by-negation pattern
   (`never a bare …`, `not a bare …`, `not just a …` *used to define*) and rewrote those. **Genuine
   behavioral guarantees are kept** (`never a panic`, `never a hang`, `never a real DELETE`, `never a
   partial write`, `$ctx never a body field`) — a warning about behavior is not a define-by-negation.

2. **No `D#` decision-refs in `based-codegen` or any userland surface.** The standing rule allows `D#`
   in internal `///` doc comments, **but** (a) the user explicitly flagged `based-codegen`'s comments, so
   *all* `D#` refs were stripped from `crates/based-codegen/src/**` (108 sites across `lib.rs`,
   `client.rs`, `openapi.rs`, `sql.rs`, `sql/{dml,mutations}.rs`, `migrate*`), and (b) userland surfaces
   must never show a `D#`: cleaned the emitted SQL re-select comment (`sql/mutations.rs` + its test), two
   OpenAPI `description` strings, the `--embedded`/`openapi` clap `--help` doc-comments (`based-cli`),
   an `E0181` sema **diagnostic message** (`based-sema`), the three example `main.rs`/`README.md`,
   `docker/{Dockerfile,healthcheck.sh,README.md}`, and the regenerated `examples/*/src/client.rs`.
   Internal `///` doc-comment `D#` refs in the other crates (`based-sema`/`runtime`/`ast`/`parser`/…)
   stay — the durable rule permits them and they aid the reviewer; they are not a userland surface.

3. **Overlong `based-codegen` comments.** Compressed the long module `//!` headers and block comments to
   terse what + why (dropped design-rationale essays, step-by-step how, and WIP/history narration),
   keeping the load-bearing what. E.g. `openapi.rs`'s "why one contract not N emitters" section and
   `sql/mutations.rs`'s create-keyed/where-keyed re-select block were roughly halved without losing the
   what.

Comments/strings only — no logic changed. Gate: `cargo test --workspace --all-features` + `fmt --check`
+ `clippy` all clean; `grep` finds no `D#` in `based-codegen/src` or any userland surface; the SQLite
quickstart ran **green** (`based migrate apply` → `cargo run`, exit 0) and the MariaDB/Postgres example
clients compile (their regenerated `client.rs` diffs are comment-only).

## D75 — HTTP listener edge errors share a `code()`/`status()` registry (H7 sub-item ii)

**Context.** D71 gave the dispatch core's failures (`PlanError`/`DbError`/`RunError`) a single-source
`code()` the wire consumes. But the `http` listener's *own* pre-dispatch failures — a bad body, a
malformed `X-Based-Context` header, a drain/readiness refusal — still built their `WireResponse`s from
**scattered string literals** (`"bad_body"`, `"bad_context"`, `"draining"`, `"not_ready"`, and a
hardcoded `"database_error"` on pool checkout), the exact drift risk D71 removed from the core.

**Decision.** Introduce a private `EdgeError` enum in `based-runtime::http` — the transport-edge twin of
`PlanError`/`DbError`: `BadContext` / `BadBody(String)` / `Draining` / `NotReady(String)`, each with a
stable `code()` and an HTTP `status()` (400 for the caller-fixable body/context faults, 503 for the
drain/readiness refusals), a `Display` (the human message), `std::error::Error`, and
`From<EdgeError> for WireResponse` so the envelope is built from the registry in one place. The four
edge sites (`TrustedHeaderContext::derive`, `read_json_body`, `ready_response`'s drain + ping branches)
now yield `EdgeError`, and the pool-checkout path reuses the driver's own classified `DbError::code()`
instead of a fixed `"database_error"` literal — so a `pool_exhausted` checkout surfaces its real code
rather than masquerading as a generic DB error. Wire codes/statuses are unchanged for every existing
case (`bad_body`/`not_ready` integration tests unchanged); only the pool-exhausted checkout code
sharpens. A custom `ContextSource` still returns an arbitrary `WireResponse` (its own policy codes —
e.g. a `401` — are the plugin's, not the edge registry's).

**Why not fold these into `PlanError`.** `PlanError` is *dispatch-core* (it has no socket, no headers);
these failures exist only at the transport edge (they never reach `dispatch`). Keeping a separate
edge registry mirrors the core's convention without leaking transport concerns into the pure core.

**Verification.** `cargo test --workspace --all-features` + `fmt --check` + `clippy` all clean; a new
`edge_error_registry_maps_code_status_and_message` unit test pins each variant's code/status/message and
the `WireResponse` it builds. No live-DB behavior changed (the checkout path is still a 503; only its
`code` sharpens to the driver's existing classification), so no live-DB run was required.

## D76 — Example `main.rs` as an error-handling reference (H7 sub-item iv)

**Context (final H7 sub-item, closing D71/D72/D75).** D71 made the generated client's `ClientError` a
real `std::error::Error` with `kind()` / `code()` / `status()`, but the example quickstarts still met it
through `.expect(...)` on every call — the scenario ran, yet it taught a reader nothing about handling
the typed error surface. The three `examples/*/src/main.rs` are the first thing a user copies, so one
should double as the copyable error-handling reference.

**Decision.** Convert `examples/sqlite-quickstart/src/main.rs` to an idiomatic `?`-based `Result` flow.
`main` now returns `Result<(), Box<dyn std::error::Error>>`; every client call threads `?` (a
`ClientError` converts straight into the boxed error), and the two helpers (`place`/`get`) return
`Result<_, ClientError>`. Scenario invariants stay `assert!`/`assert_eq!` (a demo doubles as a smoke
test — a broken invariant should panic non-zero). A new **step 6** demonstrates the typed surface: it
feeds a deliberately malformed `Cursor::from_raw("not-a-real-cursor")` to `recent_orders`, then matches
the returned `ClientError` on `kind()` — asserting the `Api { status: 400, code: "bad_cursor" }` class
and reading back `code()` / `status()` — so a reader sees exactly how to branch on a failure by class
rather than by message text. The SQLite slice was chosen because it runs anywhere (bundled SQLite, no
Docker). The module doc block + README gained an error-handling section and the new expected-output line.

**Scope.** Only the SQLite quickstart was converted (it runs on `cargo run` with no live server). The
MariaDB/Postgres quickstarts still use `.expect(...)`; converting them the same way needs a live
`mariadb:11.4` / `postgres:16` to re-verify green and is left as a follow-up (their client/error surface
is identical, so the pattern transfers verbatim).

**Verification.** `cargo test --workspace --all-features` + `fmt --check` (workspace and the example's own
`main.rs`) + `clippy --workspace --all-features` + the example's own `clippy` all clean; the quickstart
ran green via `based migrate apply` → `cargo run` (exit 0), output matching the README including the new
`rejected a malformed cursor` line.

## D77 — Editor gravy names symbols, not the system (H4 still-open half)

**Context (H4, closing the half D74 left open).** Editor hover/inlay text is "gravy" — it states *what a
symbol is* when that isn't obvious from the source, and stops there. Two facts tutorialized instead: the
scope hover (`based-facts::scope_detail`) taught how the system works ("Every read and write on a governed
model is confined … a callable opts in with `scoped` or out with `unscoped`"), and the `$ctx` hover
(`ctx_fact.detail`) explained the client wire contract ("The generated client sends exactly these; each
field's type is fixed by the scope or column it binds to"). Both are spec material. Separately, the
`requires [org: -> Org]` **inlay** and the `$ctx` hover carried the same contract on the same declaration —
duplicate surfaces for one fact.

**Decision.** Trim both hovers to a one-line identity plus the concrete filter/bag, and remove the
duplicate inlay.
- **Scope hover** → `` scope `Tenant`: filter `org = $ctx.org`; governs Widget `` — the scope's name, its
  filter predicate, and the models it governs. The confinement/opt-in prose is dropped (it lives in
  auth.md).
- **`$ctx` hover** → `` request context: this query requires `$ctx` [org: -> Org] `` — the concrete bag the
  callable requires. The wire-contract sentence is dropped.
- **`$ctx` inlay dropped.** `FactKind::CtxRequirement` no longer renders an end-of-line inlay (it joins
  `Scope` in the `continue` arm of `inlay_hints`); the hover carries the bag, so there is no
  hover↔inlay duplication. The `requires […]` label is retained only for the `based facts` CLI listing.

Also fixed a define-by-negation comment in `scope_label` ("scope is written, not derived") to positive
phrasing. Facts tests updated to the concise strings.

**Verification.** `cargo test --workspace --all-features` + `fmt --check` + `clippy --workspace
--all-features` all clean.

## D78 — `based fmt` canonical formatter + `formatting` LSP directive

**Context (H2).** The one baseline editor feature C4/D68 deliberately left out: a canonical `.bsl`
formatter, and the editor's `formatting` handler that delegates to it. The worked examples
(`spec/examples/commerce`, the three `examples/*/schema`) are the de-facto style; a formatter must
converge to exactly that, so re-formatting them is a no-op.

**Decision.** A new front-end crate `based-fmt` with one entry point,
`format_source(&str) -> Result<String, Vec<Diagnostic>>`: parse to AST, then pretty-print. It is a
**pure function of the AST** (so it is canonical, not layout-preserving) except for comments, which the
lexer discards — the printer recovers them from the source text and re-emits each in its slot. Every
`.bsl` comment is a full column-0 line (before a declaration, or between a model's decorators), never
inside a body, which makes reattachment a line-range lookup rather than trivia-threading. The layout
rules were reverse-engineered from the examples and verified as a byte-exact no-op across all committed
schemas:
- **Field alignment.** A model body aligns the type column (`name:` left-padded to the longest field
  name); fields carrying an inverse ref get a *second* aligned column for the `(Model.field)` (the
  `User.invited_users`/`placed_orders` case). Modifiers get a single space (not aligned).
- **Shapes** print inline (`shape R from Org { id, name, slug }`) when that one-line form fits within a
  width budget, else one field per line with the rename `=` aligned. (Field *count* can't decide it —
  a 3-field `OrgRow` is inline while a 3-field `ProductCard` is multi-line; only width separates them.)
- **Query blocks** use clause count: ≤1 clause stays inline (`{ list Order; }`), 2 expands the block with
  the statement on one line, ≥3 breaks a clause per line. Mutations always expand, one write per line.
- **Predicates** print with minimal parentheses (precedence-aware), so redundant author parens are dropped
  while meaning-bearing ones stay.
- `asc` (the default sort dir) and redundant single-column `@index(x)` parens normalize away — all
  AST-preserving, so the output reparses to the same declarations.

Deterministic + idempotent (`format(format(x)) == format(x)`) and structure-preserving
(`parse(format(x))` equals `parse(x)` modulo spans); an unparseable file is not formattable (`Err`).

**CLI.** `based fmt [--check]` reuses manifest/glob discovery (`discover_project`); writes each changed
file in place, or with `--check` writes nothing and exits nonzero if any file differs. A file that
doesn't parse renders its diagnostics rustc-style and fails the run. Errors flow through the structured
`CliError` (usage 2 / failure 1), not `anyhow`.

**LSP.** The server advertises `document_formatting_provider` and implements `textDocument/formatting`
by returning one full-document `TextEdit`, delegating to a thin `Snapshot::format_document` wrapper over
the same `based_fmt::format_source` — one printer, no editor/CLI divergence.

**Verification.** `based-fmt` tests assert the no-op over every committed schema, idempotency +
reparse over the conformance corpus, and per-construct exact output (alignment, shape/query modes, tx,
predicate precedence, decorator-interleaved comments); an LSP test covers the no-op-then-reformat path.
`cargo test --workspace --all-features` + `fmt --check` + `clippy --workspace --all-features` all clean.

## D79 — named nested projection: a nest may reference a shape by name (Track L4)

**The gap.** A to-one/to-many nest spelled its fields inline (`placed_by { name, email }`) and the
client codegen minted an **anonymous per-parent type** (`OrderCardPlacedBy`). Two shapes nesting the
same columns of the same model got two distinct, unshareable types — a typed frontend component or a
`db→props` mapper could not be written once against "a User projection" and reused across query sites.
With a second consumer the projection's field set is a genuinely shared fact, so principle 4 (declare
once, reference by name) applies — which it did *not* for a single-consumer inline nest (author-DRY
alone was a weak case).

**Decision: option (a) — the nest references a top-level `shape … from Model` decl by name.**
`placed_by -> UserRef` expands the named shape's body in place; `->` reads "connects to", consistent
with relation decls and param-binding. Option (b) — *naming an inline nest* (`placed_by as UserRef
{ … }`) — is rejected for now: two sites share a type only by referencing one definition, which
collapses back to (a); (b) remains possible later sugar. Residual cost of (a): one shape forced to
serve two consumers with different needs overfetches for the leaner — the fix is to split into two
cheap shapes. Docs say plainly: **reference for a shared type; inline when you mean to trim**
(shapes.md).

**Pinned semantics.**
- The referenced shape's `from` model MUST equal the relation's target model — `E0133`, never a silent
  mismatch (principle 2). An unknown shape name is `E0132`. `full` is per-model and cannot be
  referenced (the reference position takes an UpperCamel shape name; `full` doesn't parse there).
- The reference is a **pure column-list expansion**: it lowers to exactly the same SQL / `nest_row`
  path as an inline nest (D55 to-one prefixed aliases / D57 to-many correlated JSON subquery). It
  carries **no independent `@scope`/soft-delete identity** — child scope + live-predicate stay
  governed by the nest context exactly as an inline nest's.
- Recurses and composes with inline nesting to any depth. A shape that transitively nests itself by
  reference is `E0134` (reported at the reference that closes the cycle), mirroring the D14 in-progress-
  stack approach; the codegen/index/emitter walkers carry the same stack so they terminate even on an
  unchecked schema.

**Landed across:** grammar (`shape_field` gains `bare_field '->' upper_ident`) + parser
(`ShapeField::NestRef`); sema (resolve + `E0132`/`E0133`/`E0134`, index-inference demand rides the
expansion; the `$ctx`/scope walkers skip a nest ref exactly as they skip an inline nest); SQL codegen
(`Select` expands the referenced body in place — emitted SQL is byte-identical to the inline nest,
unit-proven); **client emit — the payoff** (`placed_by: UserRef` / `Vec<UserRef>` referencing the named
shape's own generated struct, emitted exactly once and shared with any callable returning it directly);
OpenAPI (`$ref: #/components/schemas/UserRef`, the referenced schema registered once); `based fmt`
(`placed_by -> UserRef`, canonical in both inline and block shape layouts); LSP (the referenced name
rides the type-reference index: go-to-def, hover, find-references, rename). Worked example:
`spec/examples/commerce` `UserRef`/`OrderDetail`/`order_detail`.

**Verified live** (SQLite in-memory, the D55/D57 pattern): a `get`/`list` whose shape nests
`placed_by -> UserRef` returns the nested object; a to-many named ref returns the nested array with a
soft-deleted child excluded. Plus parser/sema (positive + all three negative codes) /codegen-SQL
(identical-to-inline) /client/openapi/fmt unit tests and a sema conformance case.

## D80 — comprehensive rename: params, `$ctx` fields, callable names, and `@was`-aware physical rename (Track H3)

**The gap.** Rename (D53) reused the D52 reference index, which only spanned *nominal* references —
model/shape type refs, `@scope`/`scoped` refs, filter calls, field-path segments, explicit inverse
pairings. Three renameable-but-uncovered kinds of symbol remained, and one correctness hazard:
- **(a) Callable params** — a `find(min: int)` param and its `$min` body uses.
- **(b) `$ctx` bag fields** — a `$ctx.org` bound in a `scope … = $ctx.org` term and used across callables.
- **(c) Callable names** — a `query`/`mutation`/`filter` name.
- **(d) Physical rename** — renaming a field/model mapped to a live DB column silently produced a
  *drop+add* migration (data loss) rather than a data-preserving `ALTER … RENAME`.

**Decision: extend `references_at`/`rename_edits`, keep the reference index precise (principle 2:
precision over recall), and tie physical rename to Track E via an inserted `@was`.**

**Pinned semantics.**
- **(a) Params are callable-local.** The rename target is the param decl's name span; references are the
  `$param` uses (empty dotted path, matching name) *within the owning callable only* — a same-named
  param in a sibling callable is untouched. Resolvable from either the decl or any use (`param_ref_at`).
- **(b) `$ctx` fields are name-keyed across the schema.** The `$ctx` bag is coherent by name (D4), so one
  field renames everywhere: every `scope … = $ctx.field` term binding and every callable-body `$ctx.field`
  use. The rewritten span is the *field segment only* (not the `$ctx` prefix). The canonical target is the
  first occurrence in file/offset order (`ctx_occurrences`/`ctx_canonical_span`). The **scope column**
  (`org:` in `scope Tenant (org: … = $ctx.org)`) and same-named model columns are deliberately left
  alone — the scope column is a *polymorphic contract name* shared by every scoped model (like a filter's
  call-site root), so a cursor-local rename cannot safely coordinate it; excluded exactly as filters are.
- **(c) Callable names have no in-`.bsl` references** — a query/mutation is a wire endpoint, not referenced
  from other decls — so rename rewrites just the declaration (already reached by `decl_name_at`).
- **(d) `@was`-aware physical rename.** When the renamed field/model maps to a **live** physical
  column/table (present in the project's latest `migrations/NNNN/schema.snap`), `rename_edits` *also*
  inserts a `@was("old")` — ` @was("old_col")` appended to a field's modifiers, `@was("old_table")\n`
  as a leading model decorator — so the next `based migrate gen` emits a rename step (D67), preserving
  data. Inserted only when the physical name actually changes: skipped for a `(column …)`/`@table`
  override that decouples the physical name, for a field that already carries a `@was` (the existing one
  still names the snapshot's column — a rename chain keeps the *original* source name), for an inverse
  member (no physical column), and when there is no captured snapshot with that column/table (nothing to
  preserve). The `Snapshot` gained `schema` (the resolved `CheckedSchema`, for field→physical-column) and
  `migrations_root` (the dir whose latest snapshot is consulted).

**Back-edge stays list-not-rewrite.** As in D53, an inverse back-edge (a differently-named field that
merely pairs *through* the renamed symbol) is listed by find-references but not rewritten — `rename_edits`
rewrites only sites literally spelling the old name.

**Scope.** Rename spans the whole owning project (its manifest snapshot covers every `.bsl` file — the
cross-file model rename is unit-proven); it does not cross into a *different* manifest project, whose
schema is independent (embedded schemas resolve per-project, D40).

**Landed across:** `based-lsp/src/compile.rs` only (the server handler is unchanged since D53 —
`references_at`/`rename_edits`/`prepare_rename_range` gained the new cases). Unit tests: param
decl+local-use-only, `$ctx`-field binding+uses (scope column + model column left alone), callable-name
declaration-only, and `@was` insertion for a live column and a live table + the three skip cases
(`(column …)` override, existing `@was`, no captured migration) with the applied source reparsed to
confirm the `@was` round-trips.

## D81 — `@scope` confines a nest-only scoped child: compile-time first, runtime enforcement kept (Track H9)

**The leak (correctness/security).** A `@scope`d model reached **only** through a nested shape
sub-object — `field { … }` (to-one/to-many) or `field -> Shape` (D79) — was *not* confined by its
`@scope`, though soft-delete *was*. So a nested array/object could return rows the caller's `$ctx`
should exclude — a cross-scope read leak. Not a crash: both SQL sides skipped nests *identically*, so
they stayed aligned; the confinement was just silently absent. This contradicted shapes.md ("child …
`@scope` stay governed by the nest context"), D57, and the to-many subquery's own docstring.

**Root cause (a stale D34 invariant).** Codegen's scope predicate is gated by `scope_inject`, keyed by
model (`Select::scope_terms_for`, `dml.rs`); `scope_inject` is built from sema's `touched_query`/
`touched_mutation`, whose shape walk (`walk_shape_join`, `scope.rs`) only descended an `out = path`
reach (`ShapeField::Rename{Path}`) and *skipped* `Nest`/`NestRef` — mirrored by
`ctx::collect_joined_scope`'s `walk_shape_scope`. Correct when D34 landed (nests were deferred → no
join). Stale since D55/D57/D79 made nests emit real joins (to-one, via the `scope_join_pred`
chokepoint) and correlated subqueries (to-many, the `json_array_subquery` `WHERE`) that *have*
scope-injection call sites but were handed an empty term list. A child *also* reached via a
`where`/order/reach path was already in `scope_inject`, so the nest inherited the predicate — hence
inconsistent behaviour depending on unrelated clauses.

**Decision: compile-time is the primary guarantee; runtime enforcement is defense-in-depth (Option A).**
Two parts, in priority order.

- **Part 1 — the leak fix.** Both shape walks now recurse into `Nest` and `NestRef`, switching to the
  nested child model context (for `NestRef`, resolving the referenced shape's body via `cx.shape_bodies`
  and guarding a reference cycle with the same in-progress stack as D14/D79). A nest-reached scoped child
  therefore lands in **both** `scope_inject` (codegen emits the predicate) and `ctx_requires` (its
  `:ctx_<field>` bind is supplied). **Nesting into a scoped model now counts as *touching* it**, so a
  callable that nests into a scoped model but does not satisfy that model's `@scope` alternative fails to
  compile with **E0185** — "unscoped access fails at compile time, unambiguously; you cannot write a query
  that reads a scope you lack context for." The two walks are kept **byte-for-byte parallel** (the SQL
  sides must skip/descend identically — the alignment is load-bearing).
- **Part 2 — runtime enforcement kept.** Because Part 1 guarantees the callable carries the required
  `$ctx`, the existing mechanism binds it and injects `child.scope_col = :ctx` into the to-one nest join
  `ON` and the to-many correlated-subquery `WHERE`, exactly as D34 does for reach-joins. This is
  enforcement, *not* a fallback for missing context (missing context is now a compile error).

**Hard constraint — generated type optionality mirrors the schema only.** Scope filtering must never
widen a shape field's Rust type to `Option`. A nested to-one is `Sub` iff its relation/FK is
non-nullable, `Option<Sub>` iff the relation itself is nullable; a nested to-many is always `Vec<Sub>`.
The client derives this from the relation's own nullability (`client::to_one_relation`'s `optional`
flag) — never from anything scope-related — and the H9 change lives entirely in sema, touching no
codegen type path. The runtime scope predicate can in principle NULL a non-nullable to-one *only* when a
genuine cross-scope FK exists (a data-integrity violation); that surfaces as a decode error on the
non-optional field — it must not soften the generated type to `Option`. Asserted by a codegen unit test
(non-nullable nest into a scoped child → `Sub`, nullable → `Option<Sub>`, to-many → `Vec<Sub>`).

**Landed across:** `based-sema/src/scope.rs` (`walk_shape_join` recurses; `nest_target` helper) +
`based-sema/src/ctx.rs` (`walk_shape_scope` recurses; parallel) — codegen unchanged (the predicate falls
out of `scope_inject` gaining the child). **Verified:** sema (a nest-only scoped child on a divergent
axis → E0185; naming both axes → clean); codegen SQL (the scope predicate appears in the to-one nest
join `ON` and the to-many subquery `WHERE` for a nest-only child) + the type-optionality assertion; and
**live** on in-memory SQLite (a properly-scoped `order_by_id` nesting a to-one `contact { name }` and a
to-many `items { sku }` into children scoped on a *divergent* `Region` axis returns only in-scope
children — the out-of-region line item is absent from the array, the out-of-region contact reads back
NULL on an *optional* relation, never leaking its name). No commerce/`examples/**` fallout: their only
scoped model (`Order`/`Tenant`) nests into the unscoped `User`, so they still check clean.

## D82 — enum type: string + numeric kinds, explicit values, column + CHECK, variant navigation (Track T1)

First of the owner-approved **Track T — core DB feature parity** queue (enum → decimal/float → atomic
update exprs → aggregations/group-by/having → m2m/upsert → referential actions). Adds `enum` as a
first-class scalar type in two kinds — string and numeric — inferred from the variant values.

**Declaration + variant grammar.** `enum Name { pending, paid = "PAID", … }` — a top-level decl,
UpperCamel name, lowercase snake variants (comma/newline-separated). A variant is
`IDENT [ "=" ( STRING | INT ) ]`: its **name** is always the identifier (it yields the client's Rust
variant, go-to-def, and rename); its value, when written, is the wire/DB representation. The name shares the
**type-name namespace** with models/shapes/scopes (`E0106`). Parsed to `Decl::Enum(EnumDecl { name,
variants: Vec<EnumVariant { name, value: Option<Spanned<VariantValue>> }> })`; resolved to
`REnum { name, kind, variants: Vec<REnumVariant { name, value: EnumValue }> }` in `CheckedSchema.enums`
(+ `enum_index`). `enum` is a contextual keyword (decl head only).

**Kind inference.** A **string enum** has no int-valued variant — each variant is bare (`pending` → wire
`"pending"`) or explicit-string (`paid = "PAID"`, name ≠ value); mixing bare + explicit-string is fine.
An **int enum** has an int value on *every* variant (`low = 0, medium = 1, high = 2`) — no bare or string
variant allowed. Mixing an int variant with a bare/string one is `E0156` (ambiguous kind); two variants
sharing a wire value (two strings, or two ints) is `E0157` (ambiguous stored value); two variants sharing
a *name* is `E0104` (repeated member).

**Field usage + name-resolution disambiguation.** `status: Status` is an UpperCamel type reference. Sema
disambiguates by what the name resolves to: an *enum* classifies as a **scalar column** (`MemberKind::Scalar`
+ a `enum_name: Option<String>` marker; `ty = Text` for a string enum, `ty = Int` for an int enum, so the
DDL/migrate/client/openapi mapping follows naturally from the stored type), a *model* stays a relation/FK.
`model::classify` takes `HashMap<String, EnumKind>` so an enum-typed field never becomes a relation, and the
storage type follows the kind. Optional (`Status?`) + `default <variant>` supported.

**Values as bare identifiers (no new `Value` node).** A variant in `where`/`create`/`update`
(`where status = paid`, `create { status: paid }`) is an ordinary single-segment `Path` in the AST — always
referenced by **name**, never by raw value — the cleaner of the two options (vs. a dedicated
`Value::Variant`), since it needs no grammar/AST change. Sema resolves it *as a variant* only when the
compared/assigned column is enum-typed (`resolve::terminal_enum` + `check_enum_operand`): a non-member
(incl. a variant from another enum) is `E0154`; a `$param` still name-checks; anything else falls through
to the ordinary operand check. Codegen (`dml`/`mutations`) renders a variant against an enum column as its
**wire value** — a quoted string (`= 'PAID'`) for a string enum, a bare integer (`>= 1`) for an int enum —
so the runtime needs no enum awareness. **Ops by kind:** a string enum allows `= != in`; an int enum
additionally allows the ordered `< > <= >=` (numeric). An ordered op on a string enum is `E0158`. A field
`default <variant>` is `DefaultVal::Variant(Ident)` (kept distinct from a string so `based fmt` re-emits
`default pending`, not `default "pending"`); membership + non-enum-column misuse are `E0155`; the DDL/snapshot
default render as the variant's wire value.

**DB representation — column + named CHECK, both kinds, all three dialects.** A string enum stores text
(`VARCHAR(255)` MariaDB, `TEXT` SQLite/Postgres); an int enum stores the dialect's integer type
(`BIGINT`/`INTEGER`). Both carry `CONSTRAINT ck_<table>_<col> CHECK (col IN (…))` listing the **wire values**
(a renamed variant checks `'PAID'`, not `'paid'`; an int enum checks `(0, 1, 2)`). Chosen over DB-native
enums (MariaDB inline `ENUM`, Postgres `CREATE TYPE … AS ENUM`) for **migration simplicity**: a native enum
makes a variant add a non-transactional `ALTER TYPE ADD VALUE` / `MODIFY COLUMN`, can't remove a value, and
adds a second type map that can drift from `based gen sql`; SQLite has no native enum at all. One
representation through the existing `Dialect` type-map seam. Membership is enforced twice — the DSL layer
(E0154/E0155/E0158) at compile time and the CHECK at run time (defense in depth).

**Migrations.** The neutral snapshot records `enum(v1,v2,…)` for a string enum (its wire values) and
`enum:int(0,1,…)` for an int enum — distinct single `schema.snap` tokens, so a variant add/remove OR a
string↔int kind change is a **diffable** column type change; `neutral_sql_type` maps them to text/int
through `sql::sql_type`, and `create_table_statements` re-emits the CHECK so a from-scratch migration matches
`based gen sql`. Rendering an in-place variant change back to a per-dialect DROP/ADD-CONSTRAINT is deferred
(the minimum bar — diffs, never crashes — is met).

**Client + OpenAPI.** `based gen client` emits a real Rust `enum`: a **string enum** is serde-renamed to
the wire strings (`#[serde(rename = "PAID")] Paid`); an **int enum** carries explicit discriminants
(`enum Priority { Low = 0, … }`) + a hand-rolled `Serialize`/`Deserialize` that (de)serializes as the
integer — **no new dependency** (no `serde_repr`): `serialize_i64` on the discriminant, and a match on the
incoming `i64` back to a variant with an unknown value becoming a `serde::de::Error`. An enum-typed
field/output takes the enum type instead of `String`. `based gen openapi` emits `{type: string, enum:[…]}`
for a string enum, `{type: integer, enum:[…]}` for an int enum. A non-variant value the DB returns surfaces
as a client *decode* error, never a panic (existing typed-decode discipline).

**Editor navigation (variant nav — the "code following" gap).** Enum **type** references already rode the
type-ref index (go-to-def/find-refs/rename via `type_ref_target` incl. `Decl::Enum`); this adds **variant**
navigation in `based-lsp/src/compile.rs`, mirroring the D51/D52/D53 field patterns. Go-to-def on a variant
in value/default position (`where status = paid`, `default pending`, a write assign) resolves the LHS
column (through the AST field walk) to its enum, then matches the variant name to its declaration span (tried
*before* field resolution so a variant is never misread as a same-named column). Find-references + rename are
**enum-local** — variant uses are keyed by `(enum_name, variant_name)`, so a same-named variant in a
*different* enum is untouched (mirrors params being callable-local). Hover on a variant shows its enum
(+ value if explicit); hover on an enum type reference shows the enum decl; document/workspace symbols carry
the enum + its `EnumMember` variants.

**Runtime.** No change to the value paths — an enum column is text or integer end to end; the wire value is
the variant string/number, decoded by serde into the Rust enum on the client side.

**Diagnostics:** `E0104` (dup variant name), `E0106` (enum name collides with a model/shape/scope/enum),
`E0154` (non-member / wrong-enum variant in a value position), `E0155` (bad `default <variant>`, incl. a bare
default on a non-enum column), **`E0156`** (mixed int + bare/string variants), **`E0157`** (duplicate wire
value), **`E0158`** (ordered op on a string enum).

**Used in the example:** commerce `Order.status` stays the string enum `Status { pending, paid, shipped,
cancelled }` with `status: Status (default pending)`. Int enums + name≠value are exercised in unit /
conformance / live tests, not forced into commerce. `based check` clean; the commerce snapshot golden is
unchanged (a bare string enum → `enum(pending,paid,shipped,cancelled)`).

**Verified:** sema unit tests (string + int resolve; enum field scalar-not-relation with the right storage
type; mixed E0156; dup-value E0157; dup-name E0104; ordered-op-on-string E0158; unknown-variant E0154;
bad-default E0155; enum-vs-model E0106); codegen unit tests (DDL text+CHECK *and* integer+CHECK all three
dialects; string rename incl. name≠value + int discriminants/manual serde in the client; string- and
int-enum openapi; variant → wire-value SQL literal in `where`/`create`; snapshot kind encoding + round-trip +
from-scratch int render); LSP unit tests (variant go-to-def in value + default positions, find-references,
rename with a same-named variant in a second enum left untouched, enum type-ref go-to-def + hover); a parser
conformance case (string + int + name≠value); and **live** SQLite (create by name → wire values `"PAID"`/`2`
in the shape, string filter + *ordered* int filter each return the row, the CHECK rejects an out-of-range int
*and* a bad string) plus the full live MariaDB + Postgres suites green against the enum-carrying commerce DDL.
Spec: `spec/syntax/enums.md`.

**For the next queue item (decimal/float):** enum reused the `Scalar` variant with a side-channel marker
(`enum_name`) and set `ty` to the storage primitive (`Text`/`Int`), rather than a new `MemberKind` — keeping
the ~120 `MemberKind` match sites untouched. A new *numeric primitive* (`decimal`/`float`) is a cleaner fit
for `Primitive` + the `sql_type` map + the `prim_family` operand bucket, and unlike enum it *does* need
runtime decode arms (`SqlValue`) — the int enum flows through the existing `Int` arm, so the plan/scan/value
paths were still not touched. Budget for those.

## D83 — decimal + float scalar types (Track T2)

Second Track T item. Adds two numeric primitives: exact `decimal(p, s)` (money) and 64-bit `float`.

**Syntax + AST.** `decimal(p, s)` — precision `p`, scale `s` (`total: decimal(12, 2)`); bare `decimal` =
`decimal(38, 9)`. `float` is one type (double precision; no `double` alias yet — noted as future sugar).
Both are new `Primitive` variants: `Primitive::Float` and `Primitive::Decimal { precision: u32, scale: u32 }`
(kept `Copy` — u32 fields). The parser reads the optional `(INT, INT)` after `decimal` (`decimal_args`);
range validity is a *sema* check, not a parse one.

**Exact defaults / literals — no f64 round-trip.** `default 9.99` on a decimal previously parsed to
`Literal::Float(f64)`, which is lossy (`0.10` → `0.1`). Replaced `Literal::Float(f64)` with
`Literal::Decimal(String)`: the parser carries a fractional literal's **exact source text**, so a decimal
default/value is byte-exact through DDL, the neutral snapshot, and the runtime. A `float` literal uses the
same node (its text parses to a number where a float context needs one). `based fmt` re-emits it verbatim.

**Operand typing.** `int`, `float`, `decimal` all fold into the **numeric** family (`prim_family` in
`resolve.rs`, mirrored in `ctx.rs`/`scope.rs`): a numeric literal binds to any of them, and they inter-compare
with `= != < > <= >= in` (ordered ops allowed — they're numeric).

**Per-dialect DDL (one `Dialect` type-map seam).** `decimal` → `DECIMAL(p,s)` (MariaDB) / `NUMERIC(p,s)`
(Postgres) / **`TEXT`** (SQLite); `float` → `DOUBLE` / `DOUBLE PRECISION` / `REAL`. `sql_type` now returns
`String` (decimal is parameterized). **SQLite deliberately uses `TEXT`, not the owner-suggested `NUMERIC`:**
NUMERIC affinity converts `'9.99'`→REAL (lossy — `'0.10'`→`0.1`) and returns a number, breaking both
exactness and the JSON-string wire form; `TEXT` stores + returns the exact string. The cost is that SQLite
decimal comparison is *lexicographic* (documented in models.md + `sql.rs`); production dialects use a true
numeric type. Bind-time values fit in equal-integer-digit ranges so ordered filters still read correctly.

**Wire + client.** A decimal is a **JSON string** (`"9.99"`), lossless, never a JSON f64; a float is a JSON
number. The client emits `rust_decimal::Decimal` (by full path — a schema with no decimal never mentions the
crate, so the dep is needed only when used) and `f64`. `rust_decimal`'s **`serde-str`** feature makes
`Decimal` (de)serialize as a string globally (no per-field `#[serde(with=…)]`) — added to the generated
client's Cargo.toml (the three `examples/*/Cargo.toml`). OpenAPI: decimal `{type: string, format: decimal}`,
float `{type: number, format: double}`.

**Runtime carries a decimal as its wire string end-to-end — `rust_decimal` stays OUT of based-runtime.**
*(Amended by the D84 spike: under sqlx the runtime gains `bigdecimal` — bind-side + MariaDB decode,
exactness preserved; `rust_decimal` proved silently lossy past ~28 digits and stays out.)*
`Family::of(Decimal)` = `Text` (binds as a string, no new `SqlValue` variant); `float` = `Float`. MariaDB
returns `DECIMAL` as bytes → the exact string already. Postgres returns `numeric` in **binary** (a packed
base-10000 digit array, not its text), so a new `pg_numeric` decoder (mirroring the D61 uuid/timestamp/jsonb
binary decoders) reconstructs the exact decimal string — no float. SQLite `TEXT` round-trips the string
directly. So only the *decode* seam changed; the client is the sole place a decimal becomes a `Decimal`.

**Migrations.** The neutral snapshot encodes `decimal(p,s)` and `float` (`neutral_type`), parsed back by
`parse_decimal` in the renderer, so a precision/scale change or an int↔decimal change is a diffable
`alter column` and a from-scratch `0001_init` emits the right type.

**Diagnostics:** one new code **`E0159`** — a `decimal(p, s)` out of range (`1 ≤ s ≤ p ≤ 38`) *or* a decimal
column's `default` that isn't a decimal literal (an integer or fractional literal).

**Used in the example (honest money).** Commerce `Order.total` `int` → `decimal(12, 2)` (+ the `place_order`
param); commerce snapshot re-blessed; the three quickstarts' `total` converted, their `src/client.rs`
regenerated (importing `rust_decimal`), `0001_init` migration + Cargo.toml updated, and each **re-run green
live** (SQLite `cargo run`; MariaDB `mariadb:11.4` + Postgres `postgres:16` via Docker `migrate apply` →
`cargo run`). `based check` + `based fmt --check` clean on commerce + all three quickstarts.

**Verified:** sema (decimal/float resolve; bad precision/scale E0159; numeric-literal↔decimal/float bind;
exact-text default; ordered compare on decimal); codegen (DDL per dialect all three; byte-exact decimal
default; client `rust_decimal::Decimal`/`f64`; openapi decimal-string/float-number); runtime (`pg_numeric`
binary decoder unit test — `9.99`/`0.10`/`12345678.90`/negative/zero); and **live** SQLite (create `"19.99"`
→ exact string in the shape + a `0.10` trailing zero preserved + an ordered `>= 10.00` filter) plus the full
live MariaDB + Postgres suites green (`total` round-trips as `"500.00"`/`"99.00"`). Spec: `spec/syntax/models.md`.

## D84 — async-native execution architecture (Track N0 — design, settled before recolor code)

**Context.** Track N (PLAN.md) recolors the execution core to native async: the design partner needs
native async, streaming reads, and to plug the engine into their existing (wrapped) `sqlx_core` pools;
the Rust web-backend market is uniformly tokio. This entry is the N0 gate — the architecture settled on
paper so N1 implements a design instead of discovering one. The engine's sync virtues are
guarantees-by-construction (pure core, tx integrity trivially safe, backpressure by shape); each is
restated below as an invariant with a named enforcement (type system > test > review). The front end
(parse → fmt → sema → codegen → plan lowering) stays sync, pure, runtime-free; coloring touches
execution (`based-runtime` + binaries) only. Supersedes D20's execution model (sync + bounded worker
threads); D20's *sizing* insight — concurrency is bounded by the connection pool — still stands.

**1. sqlx is the driver layer (owner-settled 2026-07-10; principle 7 decides it).** The hand-rolled
driver stacks (`mysql`, sync `postgres` + r2d2, rusqlite wiring) retire. tx-drop semantics, per-DB
value codecs, pool health, and row streaming are hardened externals to reuse, not rebuild — deleting
our own driver code is a benefit, not a cost. MariaDB rides sqlx's MySql driver; SQLite rides sqlx's
own async SQLite driver (the blocking bridge we'd otherwise hand-roll). Scope guard: sqlx is an
**executor/pool layer only** — our SQL arrives already lowered with positional binds (`query_with`
over concrete per-DB types); no sqlx macros, no query builder, no `Any` driver. **`Db`/`Backend`
remain our traits**: sqlx types appear only inside driver impls and the BYO-pool constructors, never
in the trait surface. BYO-pool: a `Backend` constructor over a caller's existing `sqlx::Pool` (per-DB;
this is the design partner's concrete embed), sharing the codec path with our own backends.

**2. Transactions are a typestate, not methods.** `begin/commit/rollback` as `&mut self` methods leak
an open tx when a cancelled caller drops the future mid-body. Replaced by consuming ownership —
`Db::begin(self: Box<Self>) → Box<dyn Tx>`, `Tx::commit(self: Box<Self>)`; **drop without commit =
rollback-or-discard, and an open-tx connection is never returned to the pool**. A write can only
survive via `commit`, and `commit` consumes the tx, so cancellation at any await point cannot
double-write or leak — the invariant is unrepresentable, not policed. (sqlx's own `Transaction` has
exactly these drop semantics; our guards delegate.) Async `Drop` can't await: the fallback is
close-don't-pool, accepting connection churn on cancellation as the safe cost.

**3. One read path: `fetch` returns a row stream, always.** The one-shot wire response is a `collect()`
at the dispatch layer; N2's streaming wire surface consumes the same stream. Shipping `fetch → Vec` in
N1 and adding `fetch_stream` in N2 would fork the execution path permanently — designed out here.
Per-row shaping already fits (`nest_row` is per-row; keyset cursor mints off the last row seen).

**Trait sketch** (dyn-compatible; `async_trait`-style boxing accepted — a per-call boxed future is
noise against a network round-trip):

```rust
pub trait DbRead: Send {                    // shared by a connection and a tx
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> RowStream<'_>;
    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError>;
}
pub trait Db: DbRead {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError>;
}
pub trait Tx: DbRead {
    async fn commit(self: Box<Self>) -> Result<(), DbError>;
    // dropped without commit ⇒ rollback-or-discard; never pooled with an open tx
}
pub trait Backend: Send + Sync {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError>;
    async fn ping(&self) -> Result<(), DbError>;
}
```

**4. Coloring boundary enforced structurally, not by convention.** Only `based-runtime` and the
binaries (`based-cli`, `based-lsp`) may depend on tokio/sqlx/futures. A CI check walks `cargo tree`
for each front-end crate (`based-ast/parser/fmt/sema/codegen/facts/diagnostics/manifest`) and fails on
any async-runtime dependency. Front-end tests stay runtime-free; execution tests use `#[tokio::test]`.

**5. Retry × cancellation composition.** D65's bounded deadlock-retry survives: each attempt is one
fresh typestate `Tx`, so cancellation *between* attempts is trivially clean and *mid*-attempt is
covered by the drop guard — there is no double-write window because no attempt's writes survive
without its `commit`. Idempotency keys remain the cross-request retry answer, unchanged. Statement
timeouts stay server-side, applied via the pool's connect hook (sqlx `after_connect`), preserving D65
semantics. No engine-side deadline initially: callers compose `tokio::time::timeout` (safe by
decision 2); a config deadline is future work if an embedder asks.

**Engine + edges.** The `RefCell` single-connection `Engine` retires for a `Send + Sync`
checkout-per-call handle (`Compiled` + `Arc<dyn Backend>` + idempotency store; safe to `Arc` into
axum state) — the D22 embed door keeps its shape, one color change. `based serve`'s listener moves
tiny_http → axum (dogfoods the target stack; keeps `/healthz`/`/readyz`/drain, D26). The generated
client emits `async fn` methods (`Transport::call` → async); the CLI wraps at `main`
(`#[tokio::main]`); `MockDb` implements the async traits over its fixtures.

**Invariants (the elegance contract; each names its enforcement).**
- I1 — a connection re-entering the pool carries no open tx → **type system** (decision 2).
- I2 — cancellation can never double-write → **type system** (commit consumes) + the N1 acceptance
  test: drop a mutation future at every await point, assert rollback/discard.
- I3 — the front end is async-free → **CI dependency check** (decision 4).
- I4 — one read execution path → **code shape** (decision 3: no non-stream fetch exists).
- I5 — overload fails fast (bounded checkout wait → retryable 503, never a hang) → sqlx pool
  `acquire_timeout` mapped to `DbErrorKind::PoolExhausted` + the D65 live test re-run.
- I6 — D65 deadlock-retry semantics preserved → per-attempt `Tx` + the D65 crossed-lock live test.

**De-risk spike (in N1, before the bulk recolor).** A day-sized probe running our lowered SQL through
sqlx against the live suites, proving value-codec fidelity per dialect — uuid, timestamps, json, and
especially D83's exact-decimal contract: sqlx decodes Postgres `numeric` via its `rust_decimal`
feature (our hand-rolled `pg_numeric` decoder retires), so the spike must confirm the wire string
stays exact and decide whether `rust_decimal` entering based-runtime via sqlx amends D83's
"stays out of the runtime" note (likely yes — one line, exactness preserved). MariaDB-via-MySql-driver
compatibility is validated by the same run.

**Branch discipline (owner, 2026-07-10).** All Track N work lands on a single long-lived branch
(`async-native`) — including this design — merged back to `main` only at demonstrated confidence:
full gate + all three live suites + the examples green on the async core.

**Spike findings (N1, 2026-07-10).** The spike (`based-runtime/tests/sqlx_spike.rs`; sqlx 0.9 as a
dev-dependency, used strictly as executor — `query` + per-value binds; gated `docker-tests`, in the
live gate as `make ci-live-sqlx`) ran every `SqlValue` family through the exact codegen column types
on all three dialects. Per-dialect verdicts:

- **MariaDB via sqlx's MySql driver: works, three codec notes.** Connection + DDL (native
  `UUID`/`DATETIME`/`DECIMAL`/`JSON` + enum `CHECK`) + the string/i64/f64 binds all behave as with
  the `mysql` crate. (1) Native `UUID` and `JSON` result columns arrive wire-flagged with the binary
  charset — sqlx types them BINARY/BLOB and refuses a `String` decode; the codec reads raw bytes →
  UTF-8 (the value is already the canonical text). (2) `DECIMAL` is text on the wire but sqlx only
  surfaces it via a decimal type; `BigDecimal::to_plain_string()` re-renders the exact wire string
  (`Display` E-notates small values: `1E-30`). (3) sqlx sets `CLIENT_FOUND_ROWS`: `rows_affected` on
  a same-value UPDATE reports *matched* (1) where the `mysql` crate reports *changed* (0) — the
  Postgres semantics; nothing in the engine branches on the count today. Last-insert-id is unused
  (ids are app-generated).
- **Postgres: the bind strategy must change; numeric decode keeps our decoder.** sqlx transmits
  every parameter in **binary format** under a client-declared OID, so the current driver's bind
  trick (send wire text, let the server coerce like a literal) dies twice over: a `String` bind is a
  hard `42804` against a uuid/timestamptz/jsonb/numeric column, and an `unknown`-OID (705) text bind
  is rejected `22P03` (the server resolves the parameter to the column's type, then expects that
  type's *binary* form). **N1 binds native types** (`uuid::Uuid`/chrono/`serde_json::Value`/
  `BigDecimal`), which needs the bind site to know the value's primitive: `SqlValue` grows typed
  text-riding variants (uuid/timestamp/date/decimal — still carrying the wire string; parsing
  happens only inside the Postgres driver impl). The planner has the primitive at every typed bind
  site (the `Family::of(prim)` calls); the keyset cursor re-bind must carry the sort columns'
  primitives too (the plan has them); a raw-SQL *untyped* param stays a text bind — a raw query
  comparing one against a typed column writes the cast in its own SQL (escape hatch, explicit).
  Everything else round-trips exact: uuid canonical string, timestamptz to the microsecond, date,
  jsonb (value-exact; the text form is jsonb-normalized), bool/int/float/NULLs, and the `$n = 0`
  keyset-guard shape takes an i64 bind (sqlx declares int8 — no width inference to fail).
- **Decimal (the headline): neither sqlx decimal feature's *decode* yields the wire string on
  Postgres; the hand-rolled `pg_numeric` decoder survives the recolor.** `rust_decimal` is
  disqualified outright: at `decimal(38, 9)` it decodes without error and **silently drops** the
  fractional digits beyond its 96-bit (~28-digit) capacity, on the Postgres binary wire and the
  MariaDB text wire alike. `bigdecimal` decodes value-exact at all 38 digits but takes its scale
  from the wire's base-10000 digit groups, not the column's display scale (`0.10` → `"0.1000"`).
  The raw wire bytes, however, pass through sqlx untouched (`try_get_raw().as_bytes()`: header +
  digits + display scale verified byte-exact), so N1 keeps `pg_numeric` decoding them to the exact
  string. The mandated feature is **`bigdecimal`**, for the bind side (value-exact; the column's
  typmod rescales storage) and the MariaDB decode (`to_plain_string`). SQLite `TEXT` round-trips the
  string by construction. (Bounds check: `decimal(38, 38)` — max precision *and* max scale — is
  accepted and exact on all three dialects; MariaDB has allowed scale past MySQL's 30-cap since
  10.2, so D83's `s ≤ 38` bound is dialect-safe.)
- **SQLite via sqlx's async driver: works.** File DB, every family exact (text-riding strings
  verbatim, timestamp micros intact, bool as 0/1). Build notes: `libsqlite3-sys` is a cargo `links`
  singleton, and sqlx ≥ 0.9 specifies it as a version *range* (`>=0.30.1, <0.38`) precisely so it
  can share rusqlite 0.37's copy — sqlx 0.8 would force a rusqlite downgrade, so 0.9 is the floor
  (it also raises the build toolchain to rustc ≥ 1.94; crate `rust-version` is unaffected).

No finding invalidates the architecture above; the one trait-adjacent amendment is `SqlValue`
gaining typed text-riding variants so the Postgres driver can bind native types. (Running the full
gate also surfaced a `make check` sequencing bug, fixed in passing: the live suites reset per test
at *start* and leave their last schema + migration ledger on the shared throwaway DB, while each
example scenario expects an empty database — `check` now re-freshes the servers between the two
phases, the isolation CI already gets from one service container per job.)

**N1 implementation notes (2026-07-11; the recolor as built, where it refines the above).**
- *Deadlock retry = fresh checkout per attempt.* sqlx's transaction guard owns the pooled
  connection (commit/drop returns it to the pool internally), so the bounded retry loop cannot
  re-run on the same connection; each attempt checks a fresh `Db` out of the `Backend`. Refines
  D65's "re-run on the same connection" — semantics identical, cost is one extra checkout on the
  rare retry. Consequence: `run_mutation`/`dispatch` take `Backend` + shard key (checkout-per-call
  lives in dispatch), which is also what made the `Send + Sync` engine handle trivial.
- *Typed binds for untyped params.* The spike note "the planner has the primitive at every typed
  bind site" holds, but for *untyped* params the primitive is resolved at plan time from the
  schema: a query param through its `-> edge`/`op col` binding (else its same-named member) on the
  target model; a mutation param through the first column its `$name` fills or filters in the
  write body (named-filter calls resolved positionally). Unresolvable (raw-SQL) params stay
  shape-coerced text binds, as decided.
- *Postgres NULL binds are unknown-OID.* A `SqlValue::Null` binds as an `unknown` (OID 705) NULL —
  the server resolves it to the target column's type like a bare NULL literal (proven live:
  optional-param inserts + first-page keyset NULLs against uuid/timestamptz/numeric/jsonb columns).
- *sqlx 0.9 gates dynamic SQL* (`SqlSafeStr`): the runtime's machine-lowered, positional-bind-only
  SQL passes through `AssertSqlSafe` at the driver seam — the audit that marker asks for is the
  compiler pipeline itself.
- *`based serve` lost its worker-count knob*: with the async listener, concurrency is bounded by
  the pool (`--pool-max` + checkout timeout), completing D20's sizing insight; a separate worker
  ceiling had nothing left to bound.
- *Drain window.* axum's graceful shutdown stops **accepting** the moment it triggers — under the
  old worker model the drained workers kept answering `/readyz` 503, which is the half of D26's
  contract a load balancer actually drains on (a probe must *observe* the failing readiness; a
  refused connection is indistinguishable from a crash). `Handle::shutdown` now flips the flag,
  holds the listener open for a fixed 1s `DRAIN_WINDOW`, then triggers the axum shutdown —
  readiness observably fails first, in-flight requests still never cut off.
- *Keyset `id` tiebreaker binds as the model's own id type.* The implicit tiebreaker hardcoded
  `Primitive::Id` (a uuid-typed bind under sqlx's native-typed Postgres parameters); a model
  declaring `id: text` then failed page 2 live ("invalid uuid"). `build_order` now takes the
  tiebreaker's primitive from the model's `id` member, falling back to `Primitive::Id` for the
  implicit column. Caught by the live Postgres keyset suites — invisible to the old
  coerce-wire-text driver, which is exactly the bind-typing risk the N1 spike flagged.
- *The I2 acceptance gate, as built (`tests/cancel_safety.rs`).* "Every await point" is realized
  at the driver seam: the mutation path's await points are exactly its driver-seam calls
  (checkout, begin, each execute, the re-select fetch, commit), so a gate wrapper over the live
  SQLite backend numbers each call and parks the future there — once just before the op, once
  just after it completes (effect happened, result withheld) — and the test drops it at every
  such point. Await points *inside* one driver call are sqlx's cancel-safety, reused per
  principle 7, not re-proven. Invariants asserted per drop on the same single-connection pool:
  all-or-nothing row state (writes survive only a drop after the completed commit), the recycled
  connection is in autocommit (an explicit `BEGIN IMMEDIATE` probe — a leaked open tx fails it),
  and the pool serves the next mutation green. File-backed SQLite on purpose: the invariant
  permits recycle *or* discard of the cancelled connection, and the data must survive either.
- *Cancellation must release an idempotency claim (found by the I2 gate).* `run_mutation` only
  `abandon`ed a claimed key on the write's `Err` path; a caller dropping the future mid-write
  left the key `InFlight` forever, turning every retry into a 409 Conflict. The claim now lives
  in an abandon-on-drop guard, disarmed only once the response is recorded — so cancellation at
  any await point frees the key for the retry. A drop while the commit itself is in flight has
  an unknown outcome; releasing there matches the existing failed-commit semantics for a
  non-durable store (the durable store that resolves the claim atomically with the transaction
  remains the deferred multi-instance item).
- *BYO pool (the design-partner embed), as built.* `ShardRouter::from_pool(MySqlPool)` /
  `PgRouter::from_pool(PgPool)` / `SqliteBackend::from_pool(SqlitePool)` construct the
  `Backend` over a caller's **existing** sqlx pool (cheap-cloned — a pool is an `Arc`
  internally; one physical shard, the pool is the database), sharing the codec/tx path with
  the URL-built constructors. Contract: **their pool, their settings.** The engine applies
  nothing to a supplied pool — the session statement timeouts our own constructors install
  ride `after_connect`, which only a pool's *builder* can set, and silently reconfiguring
  sessions the app's own queries share would be wrong even if sqlx allowed it. Sizing,
  `acquire_timeout`, and connect hooks are all the caller's; a saturated pool still
  classifies `PoolTimedOut` → `PoolExhausted` (fast 503 — sqlx's default acquire wait is
  30s), and deadlock retry is unchanged (each attempt is a fresh checkout, valid against any
  pool). A caller wanting the engine's session hardening sets the equivalent `after_connect`
  on their own pool options. Proven live on MariaDB + Postgres
  (`byo_sqlx_pool_backs_the_engine`: the app's own sqlx queries and the engine's joined
  scoped read + transactional mutation interleave on one pool) plus a SQLite unit twin
  (write-through visibility both directions on a single pinned connection). This closes N1.

## D85 — streaming reads: `-> stream` signature, NDJSON wire, `Stream` client method (Track N2 — design)

**Context.** N1 made the read path stream-first by construction (D84 decision 3: `fetch`
returns a sqlx-backed row stream on all three dialects; the one-shot response is a
`collect()` at dispatch). N2 surfaces that stream to the user — the design partner's
immediately-wanted feature. This entry is the N2 spec gate; the userland contract lives in
`spec/syntax/streaming.md`, this records the choices and why.

**1. Opt-in is a signature return form: `-> stream Shape`.** Streaming changes the
client-facing contract — the generated method returns a `Stream`, the wire body is NDJSON —
and the signature is where the contract lives (queries.md: "the signature is what the
client surface is generated from"). So it is spelled as the third return-cardinality,
alongside `-> Shape` / `-> Shape[]`; grammar: `ret_type = 'stream' type_ref | type_ref
[ '[]' ]`, `stream` contextual (return-type position only). Rejected alternatives:
- *a decorator* (`@stream`) — decorators mark schema/behavioral roles; this is the return
  contract itself, and burying a type change in a decorator splits the contract across two
  places (principle 4).
- *a size threshold* — a contract that flips on data volume is consequential-by-omission
  (principle 2) and makes the generated return type undecidable at codegen time.
- *a caller-side choice* — closed RPC (calling.md): one signature = one method = one wire
  shape; a per-call flag forks the envelope and the generated types. A query wanted both
  ways is two declared queries (one source of truth per contract).
No `[]` after the shape (`stream` already means many — writing both would double-encode
cardinality, and `stream X[]` simply does not parse). Body verb stays `list` (a stream is
a list delivered incrementally; the contract/instruction split of queries.md is preserved);
`get` on a stream signature is **E0200**. `page` on a stream query is **E0201** (a page is
bounded random access + a re-entry cursor; a stream is one unbounded forward pass — the
envelopes contradict; keyset remains the UI answer, streaming the export answer). A
mutation return never streams — **E0202**. Everything else composes unchanged: filters,
param bindings, named filters, the full sort cascade + nondeterministic-order lint, index
lint / `unindexed`, shapes (including named-shape nests).

**2. Per-row nesting: materialize within the row.** Each streamed item is exactly one
element of what the `[]` form's `rows` array would be — the D57 correlated-subquery JSON
aggregation already delivers a to-many nest as one column *of its row*, so per-row
materialization is what the SQL does naturally; no new lowering. Consequence, spec'd
plainly: streaming bounds the number of rows held at once (one), not the width of a row —
an unbounded to-many child still buffers per row, and the mitigation is a trimmed shape
(no engine-imposed nest limit; principle 5: the compiler cannot know the caller's memory
budget, and a silent truncation would be worse than the buffer).

**3. Wire = NDJSON with a mandatory terminal line, not a chunked JSON array.** Same route
(`POST /q/<name>`), `200` + `Content-Type: application/x-ndjson`, each line a single-key
envelope: `{"row":{…}}` per row, then exactly one terminal `{"done":{"rows":N}}` (success)
or `{"error":{code,message}}` (mid-stream failure, same envelope as the non-streaming
error body — D71's single code registry). The status line is spent once the body starts,
so the terminal line is the only honest place for a late DB error — and **a body that ends
without a terminal line is defined as truncation** (client must report a transport error,
never completion). This is why NDJSON wins: lines parse standalone (any JSON parser,
`curl | jq`, LLM tooling), and it has an in-band place for the error/success signal — a
chunked JSON array needs an incremental parser and a truncated array is indistinguishable
from a mid-stream abort. Envelope-per-line (not bare row objects) because a bare shaped row
could collide with any sentinel key a shape might legitimately project. Pre-body failures
(unknown query, bad args, missing/bad `$ctx`, scope rejection) keep real HTTP statuses —
the server validates and plans before emitting the first byte. `done.rows` doubles as an
integrity checksum for tooling.

**4. Client surface: same method name, two-layer `Result`.** One signature = one method,
so no `_stream` suffix (there is no non-stream sibling to collide with). Shape:
`async fn name(input, ctx) -> Result<RowStream<Shape>, ClientError>` where the stream
yields `Result<Shape, ClientError>` per item — outer `Err` = the call never started
(transport / pre-body rejection), per-item `Err` = the in-band `error` line (kind `Api`,
the server's stable code) or truncation (kind `Transport`); after an `Err` item the stream
is finished. **Drop = cancel**: dropping the stream abandons the read and releases the
connection — safe by D84 (reads hold no tx; sqlx owns mid-protocol cleanup). The generated
`Transport` trait gains a streaming call beside `call` (emitted with the module, so the
orphan-rule story of D62 is unchanged); the HTTP transport parses NDJSON lines, the
`Embedded` transport consumes the engine's row stream in-process (same typed items, no
socket, no NDJSON round-trip). Generated client code is user-side async and may use
futures/tokio types — the D84 coloring boundary constrains the *compiler* crates, which
only emit this code as text.

**5. Nothing bypassed, by construction.** A stream query is the same lowered SQL through
the same single read path (D84 I4) — scope acknowledgement (E0182/E0185) applies to stream
signatures identically, soft-delete injection is in the SQL itself, `$ctx` validation
happens before the first row. There is no second execution path to audit.

**Deferred to the implementation slices (tracked in PLAN.md N2):** the runtime streaming
dispatch surface (a public engine method yielding shaped rows + the axum NDJSON body with
drain/cancel behavior), the generated client stream method + `Transport` extension +
embedded bridge, OpenAPI emitter update, parser/sema for `stream` + E0200/E0201/E0202,
fmt/LSP awareness, and the acceptance gates (mid-stream error line observed live;
drop-mid-stream releases the connection; truncation → transport error).

**N2 implementation notes (2026-07-12; the client slices as built, where they pin what the
design left open).**
- *`RowStream` spelling.* Emitted **in the generated module** (not exported from based-runtime —
  the module stays self-contained, D62's orphan story unchanged):
  `pub type RowStream<O> = Pin<Box<dyn futures_core::Stream<Item = Result<O, ClientError>> + Send>>`.
  `futures_core` is referenced by full path, so — like `rust_decimal` — the consumer needs the
  dependency only when the schema uses the feature.
- *Streaming surface is conditional.* The `RowStream` alias, `Transport::call_stream`, the
  NDJSON decoder, and the embedded streaming bridge are emitted only when the schema declares a
  `-> stream` query; a schema without one produces byte-identical output to the pre-streaming
  emitter (the quickstart clients needed no regeneration).
- *One framing decoder, emitted with the module.* `decode_ndjson(body) -> RowStream<O>` takes
  any stream of byte chunks and owns the whole framing contract — line reassembly across chunk
  boundaries, in-band `error` → typed `Err` item, EOF without a terminal line → truncation
  `Err` — so an HTTP transport is a few reqwest-shaped lines and cannot get the contract wrong.
- *The in-band `error` item carries status 503.* `ClientError::api` wants an HTTP status, the
  spent `200` would be a lie, and every mid-stream failure is a `DbError` — which maps to 503
  whenever it happens before the body. Both transports (NDJSON and embedded) emit the same
  status, so a caller's error handling is transport-invariant.
- *`done.rows` is enforced, not advisory.* The decoder counts rows and a terminal `done` that
  disagrees yields a transport-kind `Err` (a lost line inside an intact body would otherwise
  pass silently — "no terminal line = failure" extended to "wrong count = failure").

## D86 — flagship axum example: domain, architecture, coverage map, syntax-appeal audit (Track N3 — design)

**Context.** N3 builds the re-pitch artifact: a nontrivial axum service consuming the typed async
client end to end, at quickstart-DX polish, paired with a syntax appeal pass over every surface it
shows. This entry is the N3 design gate — domain, dialect, auth story, route surface, and the
feature-coverage plan settled before example code, plus the appeal audit's verdicts. Coverage policy
(owner, 2026-07-10): the three quickstarts stay minimal; this example is the total-feature-coverage
vehicle.

**1. Domain: a multi-tenant support desk (`examples/axum-helpdesk`).** Orgs, agents, requesters,
tickets, comments, billable time. Why this and not commerce: commerce is the spec's worked reference,
and the flagship must show the language *generalizes* (Track T: "commerce is only a named example");
a support desk is what a workplace backend team actually builds or integrates, and every feature falls
out of the domain instead of being decorated on:
- both enum kinds arise naturally — a string `Status { open, waiting = "waiting_on_customer",
  resolved, closed }` (with one name≠value variant) and an ordered int
  `Priority { low = 1, normal = 2, high = 3, urgent = 4 }` whose `priority >= high` is the D82
  ordered-comparison showcase;
- decimal + float arise as billable time (`hours: float`, `amount: decimal(10, 2)`, an agent
  `rate: decimal(8, 2)?`), not bolted-on money;
- two real audiences give the scope DNF meaning: agents see the org (`scoped Tenant`), requesters
  see their own tickets (`scoped Requester`) — `@scope Tenant` + `@scope Requester` stacked on
  `Ticket` is a genuine OR; an agent's private `DraftNote` is a genuine AND (`@scope Tenant, Author`);
- streaming is the compliance/BI ticket export; raw SQL is honestly motivated (the workload report
  needs aggregation, which the DSL defers to T4; date-interval math in an `overdue` filter term);
  soft-delete is archive/restore; `hard delete` is comment purging; `tx` + `^` is
  open-ticket-with-first-comment.

Models (~7, by-domain layout as in commerce): `Org`, `User` (role enum, rate), `Session` (bearer
token → org + user; the auth bootstrap), `Ticket` (status/priority enums, self-ref `duplicate_of` +
inverse `duplicates`, `tags: json`, `@created`/`@updated`, soft-delete, comments/time inverses),
`Comment`, `TimeEntry`, `DraftNote`. The build slice owns exact field lists; the coverage map below
is the contract.

**2. Dialect: Postgres, exactly one.** The modal axum + sqlx production pairing — the market the
async pivot targets — and the strictest driver path N1 built (native-typed binds), so the flagship
exercises what the design partner will run. One dialect keeps it an app, not a matrix; MariaDB/SQLite
stay proven by the quickstarts + live suites.

**3. Architecture + auth story.** The service *embeds* the engine — that is the pitch (an app's own
axum listener, not a `based serve` sidecar): the app builds its own sqlx `PgPool`, hands it to
`PgRouter::from_pool` (the D84 BYO-pool seam, demonstrated for real), wraps it in the `Send + Sync`
`Engine`, and every handler calls the generated embedded client. Auth-derived `$ctx`: a tower
middleware reads `Authorization: Bearer <token>` and resolves it *through the typed client itself* —
`query session_by_token(token) -> SessionCtx unscoped("auth: resolves the caller's tenant")` — into
the request's `Ctx { org, user }` extension; handlers pass that to every call. So the auth story
dogfoods the client, shows `unscoped` doing its one legitimate job, and `$ctx` is visibly *derived*,
never trusted from the body. Demo tokens come from a seed step using the client's own mutations (D63
pattern; no raw SQL).

**Route surface** (~12 routes, three audiences):
- requester portal: `POST /tickets` (tx create + first comment, Idempotency-Key honored),
  `GET /my/tickets` (`scoped Requester`), `POST /tickets/:id/comments`;
- agent desk: `GET /tickets` (search: named filter + `~` + enum `in`, keyset page, `order`
  override), `GET /tickets/:id` (nested detail: `requester -> UserRef`, ordered `comments`
  to-many, time entries), `GET /queue` (`priority >= high`, `assignee = $ctx.user` Handle-1),
  `POST /tickets/:id/assign` + `/close` (guarded), `POST /tickets/:id/time` (float + decimal),
  `DELETE /tickets/:id` + `/restore` (soft-delete round trip), drafts (AND scope);
- ops/finance: `GET /export/tickets.ndjson` (`-> stream`, re-served through axum),
  `GET /reports/workload` (raw SQL aggregation), `GET /admin/tickets` (offset + `with count`,
  `unscoped("ops: cross-org support")`), comment purge (`hard delete`).

**4. Feature-coverage map** (feature → where it appears):

| feature | site |
|---|---|
| models, nullable/defaults, implicit id | every model |
| forward + inverse + self-ref relations | `Ticket.assignee?`, `Ticket.comments`, `duplicate_of`/`duplicates` |
| string enum (name≠value) / int enum + ordered op | `Status.waiting` / `Priority`, `/queue` `priority >= high` |
| decimal / float | `TimeEntry.amount`, `User.rate` / `TimeEntry.hours` |
| soft-delete, restore, `hard delete` | ticket archive/restore, comment purge |
| `@created` / `@updated` | `Ticket` |
| sort cascade (model / relation / query) | `Ticket @sort` / `comments @sort(created_at asc)` / search `order` |
| `@index` bare + composite + unique, W0103/W0104 clean | `Ticket`, `Session.token (unique)` |
| `unindexed(...)` annotation | the export / audit scan |
| named scope, OR-DNF, AND-DNF, `scoped`, `unscoped` | `Tenant`+`Requester` on `Ticket`; `Tenant, Author` on `DraftNote`; auth + admin routes |
| Handle 1 (`$ctx` in `where`) / Handle 3 (`guard`) | `/queue` `assignee = $ctx.user` / `close_ticket guard caller_can_close` |
| bare / per-param (`->`, `op col`) / full-body queries | `ticket(id)`; `tickets_for(agent -> assignee)`, `since: timestamp > created_at`; search |
| param defaults | `status: Status = open` on search |
| named filters, `~`, `in`, `has`, `not`/`or` | `filter open_states`, search, `tags has $tag` |
| shapes: bare, reach-rename, inline nest, named ref | `TicketRow`, `requester_name = requester.name`, detail nests, `-> UserRef` |
| keyset / offset + `with count` | search / admin listing |
| `-> stream` + NDJSON + client `RowStream` | the export route |
| raw SQL: value, predicate term, whole query | an export shape's raw `age_days` value, the `overdue` interval predicate term, the workload report |
| mutations: create/update/delete/restore/tx+`^` | open (tx), assign/close (update), archive/restore |
| idempotency-keyed writes | `POST /tickets` + Idempotency-Key |
| typed ids, `Cursor`, `ClientError` mapping | handlers end to end |
| migrations + `@was` rename + drift verify | `0001_init`, `0002` renames a field via `@was` |
| BYO pool + embedded client + async engine | `PgRouter::from_pool` + `client::embedded` |

**Deliberate exceptions** (coverage forced in would damage the pitch; owner can veto): `based serve`
+ the container image (the flagship embeds — serving stays covered by quickstarts, docker/, CI);
the legacy-DB affordances `(column "…")`, `@table("…")`, `(on: …)` (a greenfield app would have to
fake a legacy database to show them — they stay spec + conformance covered; `@was` *is* shown);
`shape full` (a naming convention, adds no surface the named shapes don't show).

**5. Syntax-appeal audit.** Verdict list over every surface the example shows. Headline: **zero
grammar changes** — consistent with the pitch feedback ("the syntax landed well"); what needed
polish was worked-example/prose drift, and the real gaps the example surfaces are runtime seams
(§6), not spellings.

*Polish now (landed with this entry):*
- commerce shape-name drift: the shared User projection was renamed `UserDetail` while its own
  comments, shapes.md, and D79 all say `UserRef` — renamed back to `UserRef` (order/model.bsl,
  user/model.bsl, the LSP nest-reference test).
- pagination.md's example block predated the delimited-clause grammar (`list User by active order
  created_at desc page 20` — no `by` exists; clauses take parens) — rewritten to canonical forms.

*Accept as-is (each with why):*
- `@soft_delete(deleted_at)` + the declared tombstone field — the double-spell *is* the visible
  contract (principle 2); the pairing is a pitch point, not noise.
- explicit param types on mutations / full-body callables (`total: decimal(12, 2)`) — the signature
  is the wire contract; explicitness there is documentation, and the bare form already covers the
  low-ceremony case.
- `= null` (no `is null`) — one operator set everywhere; SQL's null special case is the wart.
- `^` tx back-ref — single-purpose, always adjacent to the create it references; hover explains it.
- `@index field` vs `@index(a, b)` dual spelling — the bare form reads better for one column; the
  example adopts the convention bare-single/parens-composite rather than the grammar losing a form.
- `unscoped("reason")` wordiness — the loudness is the feature (principle 6).
- `scoped Name` after the return type, `-> stream Shape`, `field -> Shape` named nests, bare enum
  variants in value position — all land at first look; no change.

*Deferred:* none carrying syntax. (The D57 "to-many element order unspecified" caveat graduates
from documented-limitation to build-slice prerequisite, below — it is a lowering gap, not a
spelling.)

**6. Prerequisites the design surfaced (build slices, tracked in PLAN.md N3):**
- **Ordered to-many nests.** The ticket detail must show comments in `@sort(created_at asc)` order;
  D57 deliberately left JSON-aggregation order unspecified. All three dialects now have an ordered
  aggregate form (Postgres `json_agg(… ORDER BY …)`, MariaDB `JSON_ARRAYAGG(… ORDER BY …)`, SQLite
  ≥ 3.44 aggregate `ORDER BY`), so the sort cascade can be honored inside the subquery. Gets its own
  decision entry when built.
- **`guard` runtime seam.** Handle 3 parses (`Mutation.guard`) but nothing invokes it — auth.md
  promises "we own that the check runs." The engine needs a registered-guard registry (host async fn
  over ctx + args; deny → 403; a declared guard with no registered impl fails loudly at engine
  build), invoked by dispatch before the write on both doors.
- **Idempotency key on the typed client.** `Engine::call_with_key` exists but `Transport::call` and
  the generated mutation methods carry no key, so the wire header has no typed-surface twin — thread
  an optional key through the generated surface + both transports.

**Slice plan** (one iteration each, PLAN.md N3 is the tracker): N3a ordered nests → N3b guard +
idempotency-key seams → N3c schema/migrations/client → N3d the axum service + live smoke gate →
N3e README + final appeal re-audit on the real artifact + CI wiring.

## D87 — ordered to-many nests: the sort cascade reaches inside the JSON aggregate (Track N3a)

Supersedes D57's "array element order is unspecified" caveat, which D86 graduated to a build
prerequisite (the helpdesk ticket detail needs comments in chronological order). A to-many nest
(`items { … }`, `comments -> CommentRow`) is a *traversal*, so its array order follows the
existing sort cascade for the rows reached that way — **relation `@sort` on the edge > the child
model's `@sort` > unspecified** — with no new syntax: both tiers already parse (field-level
`@sort` was in the grammar and sema-resolved against the target model; only lowering ignored it).
Governed by sorting.md ("sort is a property of rows, not projection") + principle 2.

- **Semantics.** The nested array is the child rows as the traversal's cascade orders them. The
  query tier never reaches inside a nest — a query's `order (…)` orders *its own* rows; shapes
  still carry no sort (shapes.md rule unchanged). With no sort at either tier the array remains
  an unordered set exactly as under D57 — omission keeps its one safe meaning, and nothing
  silently invents an order. No tiebreaker is appended inside a nest (mirrors a non-paginated
  `list`): ties order per the database, ordered keys deterministically.
- **Lowering: ORDER BY inside the aggregate, one seam.** `Dialect::json_array_agg` takes the
  rendered sort keys and places them inside the aggregate call — SQLite
  `json_group_array(elem ORDER BY …)` (aggregate ORDER BY, SQLite ≥ 3.44; the bundled build is
  3.50.x), MariaDB `COALESCE(JSON_ARRAYAGG(elem ORDER BY …), JSON_ARRAY())` (10.5+; the live
  suite runs 11.4), Postgres `COALESCE(json_agg(elem ORDER BY …), '[]'::json)`. The correlated
  subquery itself is unchanged, so the D57 shape (never multiplies outer rows, composes with
  pagination) and the D84 single read path hold — `get`/`list`/re-select/`-> stream` all get
  ordered nests from the one lowering. Sort keys resolve against the child inside the subquery's
  own join scope (`resolve_from` from the `s<n>_` alias), so a dotted sort path joins inside the
  subquery, and nested to-many-in-to-many nests each carry their own ORDER BY.
- **IR.** `RMember` now carries the field's relation `@sort` terms (`sort: Vec<SortTerm>`, empty
  when undeclared) — sema already checked them against the target model; the checked schema just
  never exposed them. `to_many_edge` hands the edge's terms to the subquery builder, which falls
  back to `child.sort`.
- **Verified.** Codegen: per-dialect ORDER BY-inside-aggregate assertions, the relation-beats-model
  override, and the no-sort-at-any-tier form staying unordered. Live: children seeded out of
  sort-key order come back ordered on real SQLite (in the normal gate) and on the live MariaDB +
  Postgres suites — one test each proving the model tier (`comments` by `@sort(pos asc)`) and the
  relation-override tier (`pins: Pin[] @sort(rank desc)` beating Pin's own `asc`) in one response.
- **Deferred.** A per-nest sort override spelling (ordering one *use* of a traversal differently)
  — the helpdesk design needs only the two declaration tiers; adding a third, projection-side
  spelling would break "sort is a row property" for no shown need.

## D88 — guard runtime seam + idempotency key on the typed client (Track N3b)

Closes the two runtime seams D86 §6 flagged before the helpdesk example: `guard` (auth.md Handle
3) parsed but nothing invoked it, and the typed client had no way to send the idempotency key the
wire (D25) and `Engine::call_with_key` already accepted.

**Guard seam.** `RMutation.guard` now carries the declared name out of sema. The runtime gains
`guard::Guards` — a registry the embedding app builds (`Guards::new().register(name, async fn)`);
a guard fn receives an owned `GuardRequest { callable, args, ctx }` and returns
`GuardVerdict::Allow` or `GuardVerdict::deny(reason)` (a denial reason is mandatory — never
silent). Enforcement is a **single point**: `dispatch` runs the declared guard on the mutation
path before the write body, before the idempotency store (a denial never claims a key), and
before argument validation (a denied caller learns nothing about the request's validity) — so the
HTTP door and the in-process `Engine` door cannot diverge. Wire: deny → `403 guard_denied` with
the guard's reason. Build-time contract: `Engine::with_guards` fails (`GuardSetupError`, naming
every uncovered `(mutation, guard)` pair) when a declared guard is unregistered; `Engine::new`
stays the guard-free convenience and panics on a guarded schema; `based serve` refuses to start on
one (the standalone listener has no host code to register — a guarded schema embeds). The
request-time backstop for a raw `dispatch` is a loud `500 guard_unregistered`. Guards are
mutation-only (grammar); auth.md's Handle-3 example predated that and is fixed to a `mutation`.
Streams can't carry guards (a guard gates a write; mutations can't stream, E0202), so D85's
pre-body-status rule needs no guard case. Proven: dispatch unit tests (allow with observed
args/ctx, deny 403 + no SQL, unregistered 500, denial-never-claims-key), Engine build tests, a
listener-refusal test, and a live-SQLite guard that genuinely reads the database (allowed while
the row is open, denied after its own write closes it).

**Idempotency key on the typed client.** Every mutation method gains a keyed twin —
`place_order_with_key(input, ctx, key: &str)` — mirroring `Engine::call`/`call_with_key`, so the
common no-key call stays clean and the retry-safe call is one suffix (rather than an
`Option<&str>` on every call or a builder the common case pays for). `Transport` gains
`call_with_key` (required, no default — a transport must carry the key, never silently drop it):
the HTTP path sends the standard `Idempotency-Key` header, the emitted embedded bridge calls
`Engine::call_with_key`. Emitted **only when the schema declares a mutation** (the D85
conditional-surface pattern), so a query-only schema's module is byte-identical. The committed
generated clients (tests/support + the three quickstarts) were regenerated; the sqlite
quickstart's copy had drifted from verbatim generator output (it had been rustfmt-ed) and is now
verbatim again. OpenAPI now documents the wire it always had plus the new code: a reusable
`Idempotency-Key` header parameter on mutations, `409`/`422` keyed outcomes, and a `403` denial on
guarded mutations. Proven: real-HTTP replay through the generated client (header → one
transaction, identical bodies), the embedded twin, and codegen emission tests both ways.

## D89 — flagship re-audit on the shipped artifact + final coverage map (Track N3e)

Closes Track N3: the D86 syntax-appeal audit re-run as an outside evaluator over the *real*
artifact — every `examples/axum-helpdesk` `.bsl` file, the service code, and the call sites —
plus the final feature-coverage reconciliation against D86's map. Headline: **zero grammar
changes upheld.** The schema surfaces D86 judged on paper hold up in situ — the scope
declaration/reference pair, stacked-`@scope` OR vs one-line AND, `priority >= high` on an int
enum, `-> stream`, `unscoped("reason")`, `guard`, `field -> UserRef` nests, and the bare query
form all read at first look exactly as pitched. What the paper audit missed is small, and none
of it is a spelling the grammar should change.

**In-situ verdicts (new findings, each accepted or promoted):**
- **Per-param `op column` binding reads side-ambiguous on first contact.**
  `since: timestamp > created_at` lowers with the **column as the left operand**
  (`created_at > $since`), but a first read can parse it as `$since > created_at` — the
  opposite inequality. The shipped schema glosses it in a comment, and queries.md now states
  the operand rule explicitly (doc fix landed with this entry). Accepted, no respelling: the
  fragment is `op column` ("binds `>` against `created_at`"), the doc line makes it
  single-meaning, and a mirrored spelling would silently flip the meaning of every existing
  schema. Re-open only on repeated design-partner misreads.
- **`query my_tickets() -> TicketRow[] scoped Requester { list Ticket; }` carries a body the
  bare form doesn't need** (verified: the zero-param bare form checks clean). Accepted: the
  explicit `list Ticket` names the target model at the portal's entry query, and five sibling
  queries already demonstrate the bare form — the redundancy is documentation, not noise.
- **Zero-param callables still take a unit input struct** (`api.my_tickets(client::MyTicketsInput,
  ctx)`). Generated-surface ceremony, not grammar. Accepted: the uniform `(input, ctx)` shape
  keeps every call site the same and params can be added without changing call arity; revisit
  only if partner feedback names it.
- **Enum params ride the wire by value, not variant name** (`?status=waiting_on_customer`
  where the schema spells `waiting`). Accepted: that *is* the enums.md contract — name is the
  source-level spelling, value is the stored/wire spelling — and the example's smoke asserts it
  deliberately.
- **`~` is verbatim LIKE**, so the search handler wraps `%…%` itself. Accepted (the operator
  stays honest about SQL); queries.md now says so in one line, and the example README notes it.

**Final coverage map (deltas vs D86 §4; everything not listed shipped as mapped):**
- **`in` — not demonstrated.** The operator exists but only as `col IN ($param)` (one bound
  value, degenerate); there is **no literal value-list form** (`status in (open, waiting)` does
  not parse), so the search query composes `not (status = resolved or status = closed)` instead.
  Promoted to a PLAN item.
- **Raw whole query — not demonstrated; raw-at-leaves shipped instead.** Whole-query raw bodies
  (raw.md's third level) are specified but not implemented. The workload report uses raw
  correlated-subquery **value leaves** over an engine-owned row set — arguably the better raw.md
  story (the engine keeps scope/soft-delete/ordering; the raw text owns only itself). PLAN item:
  implement the third level or amend raw.md.
- **Comment purge route omitted.** `purge_comment` (the `hard delete` site) is declared in the
  schema — the syntax is shown — but no route calls it: a `hard delete` with a declared return
  shape is undecodable through the typed client (the wire returns `{}` per the delete-shape rule;
  the client expects the shape), and the grammar's mandatory `-> ret_type` on mutations leaves no
  legal shapeless spelling. Promoted to a PLAN item (sema-reject the pairing, re-select
  pre-delete, or allow a shapeless return).
- **Three additive triage mutations** (`set_status`, `tag_ticket`, `mark_duplicate`) beyond the
  D86 route list — the desk needed them; each is an ordinary scoped update, no new surface.
- **`with count` declared but `total` unserved.** The generated `Page<T>` has no `total` field,
  so `/admin/tickets` serves rows + cursor only. PLAN item.
- Deliberate exceptions (D86: `based serve`/image, `(column …)`/`@table`/`on:`, `shape full`)
  stand unchanged.

**Runtime/library gaps the build surfaced (not syntax; promoted to PLAN.md "Track N follow-ups"
with symptom/seam/fix each):** the `in` value-list form; whole-query raw reads; guard re-entry
deadlock (`Engine::call` holds the id-gen lock across dispatch, so a guard can only use a
*second* engine — auth.md now tells that truth; narrow the lock); `hard delete` + declared shape
undecodable; `Page<T>` missing `total`; zero-row update → `200` null body → client decode error
(deserves a `not_found` outcome).

**Docs landed with this entry:** the example README (`examples/axum-helpdesk/README.md`, every
command run-verified live), queries.md operand-side + verbatim-LIKE lines, the auth.md guard
re-entry correction, and the CI workflow's examples-job comment now naming the helpdesk smoke
(the job itself already ran it via `make ci-examples`).

## D90 — signature param bindings are first-class editor references + binding hover (NF15)

The owner-observed hole (2026-07-16, on `tag: json has tags`): a signature binding's column/edge
ident was collected nowhere — the LSP's `field_paths()` reference walk covered shape bodies, query
clauses, and mutation writes only — so **renaming a model field silently skipped its binding
uses** (rename `Ticket.tags` and `has tags` kept the old name: a broken schema from a refactor
that reported success), find-references had the same blind spot, and hover / go-to-def on the
binding resolved nothing.

- **Binding idents are field references.** `field_paths()` now also yields each query param's
  binding ident (`-> edge` / `op col`), rooted at the query's target model (a block statement's
  model, else the return-derived root — the same model sema checks bindings against). Go-to-def,
  find-references, rename, and the field-signature hover all come free from the existing
  reference walk; no new index.
- **Binding hover.** Hovering a binding states the predicate it generates, anchored at the
  binding itself — the column/edge ident (led by the bound field's signature) and the operator
  token between the param head and the ident: `` binds `tags has $tag` — containment
  (array/json); the column is the left operand ``. Per-op gloss: `~` SQL LIKE (pattern
  verbatim), `in` membership, `has` containment; every `op col` gloss names the column as the
  left operand — the operand rule D89 documented in queries.md, now discoverable at the cursor.
  An `-> edge` binding reads `` binds `author = $user` — via the `author` relation edge ``.
- **The derived default is discoverable.** An *unbound* param of a bare/inline query hovers as
  `` binds `name = $name` — an unbound param binds its same-named column `` (block-query params
  are `$`-referenced in the body, so no fact there). The same-name-equality convention now
  reveals itself at the moment it applies, with zero prior knowledge.
- **tmLanguage audit against grammar.ebnf** (the NF15(c) sweep): the keyword/type lists now
  match the grammar — added `scope enum guard scoped offset read sql`, the word operators
  `and or not in has` (a new `keyword.operator.word` rule), the modifiers `unique column`, and
  `float`/`decimal` to the primitive-type rule; moved `tx` to the statement row; dropped
  `model` (a model decl is bare `Name {`, D8 contextual keywords), `raw` (the `.bsl` marker is
  `sql` today — NF14 owns that rename), and `reason` (`unscoped` takes a bare string).

Out of scope, deliberately: renaming a field an *unbound* param derive-binds to does not rewrite
the param — the param name is wire contract (renaming it changes the generated client API), and
the miss is a loud sema error (`E0111`, the param maps to a column that no longer exists), not
silent corruption; the hover fact makes the coupling visible. Unit-proven in based-lsp compile.rs
(`binding_column_navigates_and_renames`, `binding_hover_states_generated_predicate`).

## D91 — derived facts anchor narrowly, never at a whole declaration (NF10)

The owner-observed bug (2026-07-16, on the helpdesk `Ticket` model): three fact kinds anchored
at whole-declaration spans (`model.span` / `q.span` / `m.span`, keyword→body-end), and the LSP
hover appends every fact whose span contains the cursor — so hovering *any* token inside the
model showed the inferred-index fact, and any token inside a query/mutation body dragged in the
irrelevant `requires […]` / `resolved query` sections.

Fix, in `based_facts::facts` (the hover handler is untouched — span containment stays the right
rule once anchors are narrow):

- **InferredIndex** anchors at the forward relation member whose FK column the index covers
  (`model.member(idx.columns[0])` — member spans are the name ident), i.e. where the write cost
  is incurred; fallback the model-name ident, then `model.span`. On the helpdesk schema the
  `inf_ticket_deleted_at_duplicate_of` fact now sits on `duplicate_of`, not the whole `Ticket`.
- **ResolvedQuery + CtxRequirement** anchor at the callable's name ident (one lookup serves
  queries and mutations — they share the wire namespace); fallback the decl span.
- **InferredInverse and Scope facts already anchored narrowly** (member span / name idents) —
  unchanged, and the find-references walk that matches on the inverse fact's span is unaffected.

Inlay placement: callable inlays render at end-of-line of the anchor, and the name ident shares
the signature line — placement unchanged. The index inlay moves from the model's first line
(a decorator line, when decorated) to the inducing member's line — where the fact is about, and
multiple inferred indexes no longer stack on one line. Regression-proven in based-facts tests
(`inferred_index_anchors_on_the_inducing_forward_edge`, `callable_facts_anchor_on_the_name_ident`,
`mutation_ctx_fact_anchors_on_the_name_ident`) and in-situ via `based facts` on the helpdesk
schema (index → `ticket/model.bsl:32:3` the member, ctx → callable name idents).

## D92 — zero-row surviving-write mutation is a 404 `not_found`, never a null success (NF6)

The flagship-surfaced bug (D89/NF6): a mutation whose `where` matched nothing — canonically a
cross-tenant id the injected `@scope` filter excludes — returned `200` with a `null` body (the
declared-shape re-select found no row, `apply_once` defaulted to `J::Null`), and the generated
client, whose mutation methods return the bare shape (not an `Option`), failed with a `Decode`
error instead of reporting the miss.

Resolution — the miss is a first-class outcome, decided at the re-select inside the transaction:

- **`apply_once` returns `Ok(None)` when the re-select reads back no row**, *before* commit — the
  transaction drops → rollback, so in a `tx` body a sibling write (e.g. an audit-log `create`)
  never survives a miss. All-or-nothing already was the mutation promise; this extends it to
  "the target row must exist". A zero-match single UPDATE wrote nothing anyway; rollback is free.
- **`RunError::NotFound(callable)`** (code `not_found`, `Display` "matched no row (no such row, or
  it is out of scope)") raised by `run_mutation` on the `None`; `serve::dispatch` maps it to
  **404** with the same stable `not_found` code the router uses. The response is identical for an
  absent row and an out-of-scope row, so existence never leaks across a scope boundary.
- **Idempotency:** a not-found releases the key claim (same path as a write failure — nothing was
  written, nothing recorded), so a retry may run once the row exists.
- **Unaffected:** a real DELETE (no re-select — returns `{}`, NF4's separate problem) and `get`
  queries (they return `Option<Shape>`; a query miss stays `200 null` by design).

Spec: mutations.md "Return shape (read-back)" now states the not-found contract; calling.md's
error-code list gains `not_found` (404). Proven: unit (`mutation.rs` rollback + `RunError`,
`serve.rs` wire 404), live SQLite (cross-tenant + absent id → 404, row unchanged), and the
helpdesk smoke gains the cross-tenant status-update → 404 assertion (previously the decode
failure that filed NF6). The helpdesk needed no route changes — its `ApiError` already passes
the engine's status + code through.

## D93 — `in` value-list form: explicit membership lists in predicates (NF1)

The flagship-surfaced gap (D89/NF1): `in` existed only as `col IN ($param)` — one bound
value, degenerate membership — so `status in (open, waiting)` did not parse and the helpdesk
spelled its open-states filter `not (status = resolved or status = closed)`.

**Grammar.** `comparison` gains a second alternative: `path 'in' value_list` with
`value_list = '(' value { ',' value } ')'` (≥ 1 element; `in ()` is a parse error). After
`in`, `(` unambiguously opens the list — a bare `value` never starts with `(` — so the
single-bind form `col in $param` keeps parsing exactly as before. Elements are ordinary
`value`s: literals, enum variants (bare identifiers), `$param` references, columns.

**AST.** A distinct `Predicate::InList { path, values: Vec<Value> }`, mirroring the grammar
alternative — not a `Value::List` (a list is meaningless in every other value position:
assigns, function args, filter args) and not a widened `Cmp` (its `value` stays single).
The single-bind form remains `Cmp { op: Op::In }`.

**Sema — per-element checking, reusing the existing codes.** The LHS path resolves as any
comparison LHS. Against an enum-typed column each bare element is membership-checked via the
E0154 machinery (`check_enum_operand` — a borrowed variant from another enum is the same
E0154); a `$param` element is name-checked. Otherwise each element is family-checked against
the column with `=` semantics (`check_in_element_type`, the per-element twin of
`check_cmp_types` step 2): `total in (1, "two")` is the same E0151 that `total = "two"`
raises; numeric-family rules per D83 (int/float/decimal inter-compare); `null` elements are
unconstrained (consistent with `= null`). No new diagnostic code was needed. Ripple walkers
extended: `$ctx` inference (a `$ctx.f` element types by the column), joined-scope reach,
index-lint eq-bucketing (`in` already counted as an equality lead), custom-join `on:` (list
allowed, request-bound elements rejected as JOIN_FORM — same as `Cmp`).

**Lowering + runtime.** All three dialects emit `col IN (v, v, …)`: variants as their wire
values (`IN ('open', 'waiting_on_customer')`, integers for an int enum), literals per-dialect,
`$param` elements as their own named placeholders — the existing positional rewrite +
bind machinery handles them (a mutation `where` types the param by the column via
`param_use_in_pred`). Named-filter inlining substitutes bound args through list elements
(`subst_pred`). No runtime code paths changed beyond param typing.

**Editor surface.** In-list variants are first-class variant uses (go-to-def /
find-references / rename ride `pred_variant_sites`), the LHS and column elements are field
references (`pred_paths`), `$param` elements are param references, and `based fmt` emits the
canonical `status in (open, waiting, $extra)` (deterministic + idempotent + reparse-stable).
The tmLanguage already word-scoped `in` (D90).

**Used in the example.** The helpdesk `open_states` filter is now
`not status in (resolved, closed)` — the D89 symptom site — keeping its "excluding the
terminal states stays correct when an active status is added" property.

**Verified:** parser unit (list vs single-bind AST forms, empty-list error) + sema unit
(enum membership incl. cross-enum E0154, family E0151, `$param` elements, unknown-param
E0113) + a sema conformance golden (`in_list`); codegen unit — MariaDB/Postgres/SQLite
string-enum wire values + `$param` placeholder + int-enum integers + numeric literals; fmt
canonical/idempotent case; LSP in-list variant go-to-def; **live** SQLite (seeded three
statuses, `status in (pending, $extra)` with `$extra = "PAID"` returns exactly the two
listed rows); full `make check` green (all three live suites + all example scenarios).

## D94 — whole-query raw bodies: the third raw level, implemented (NF2)

raw.md always specified three raw levels; the third (whole query / raw join) had no
grammar or implementation — the flagship's workload report substituted raw value leaves
(D89). NF2's default path was to implement the contract, and nothing in the
architecture resisted it: the lowering, binding, and decode seams all already treat a
query as "one SQL text + named placeholders + rows decoded by output alias".

**Grammar/AST/parser.** `query_block = '{' ( statement | raw_body ) '}'` with
`raw_body = raw_sql ';'` — the existing `sql` backtick block (NF14 owns the marker
rename) in body position, mirrored as `QueryBody::Raw(RawSql)`. The `;` is optional in
the parser (same leniency as a statement's).

**Sema — keep exactly what the hatch promises, reject the rest loudly (P6).** The
target model still comes from the return shape's `from`; verb from signature
cardinality. New codes E0210–E0214: params must be explicitly typed and unbound
(E0210 — no column to infer a type from, no engine-built WHERE for a binding to
ride); `${ctx.…}` has no type source (E0214 — a typed param is the spelling; note the
pre-existing raw *leaves* silently contribute nothing to `ctx_requires`, a latent
UnboundPlaceholder at run time — out of scope here); `scoped` on a raw body is E0211
(the engine can't inject scope into SQL it didn't build — a scoped target needs
`unscoped("reason")`, and E0182 still forces that choice); `-> stream` is E0212
(bounded slice; nothing in the streaming dispatch fundamentally resists it — lift
later if a partner needs it); a nested return shape is E0213 (nests lean on
engine-built join aliases / JSON aggregation; the shape must be flat — `out = sql`…``
leaves are fine, they're just typed columns). Skipped for raw bodies: E0144 (`get`
unique-keying — the SQL owns it), W0100 (the SQL owns ORDER BY), the W0103 index
analysis (`opaque`, same treatment as a raw predicate atom). **W0102 widened:** the
lint fires on the target model *and* on any other `@soft_delete` model whose table
name the raw text mentions (identifier-boundary match) — raw.md's joined-table
example, detected not just asserted.

**Lowering/runtime.** `lower_query` short-circuits: the rendered raw text (params →
`:name`, `{table}`/`{id}` → the target's quoted table/key) IS `LoweredQuery.sql`,
trailing `;` normalized; no count/keyset. The planner needed zero new code — params
bind by their (mandatory) annotations through the existing env, the envelope follows
the verb. Rows decode by column alias exactly as engine-built rows do, so the shape
contract is "produce these column names".

**Client/OpenAPI.** The generated method/input/response are indistinguishable from an
engine-built query's (that is the point of the typed surface). One correction: the
same-name param→entity convention (`Id<entity::M>` inference) is switched off for raw
bodies — their params are pure bind values, typed by annotation only.

**Editor/fmt.** fmt reprints the backtick interior byte-exactly (single-line raw
inlines like a one-clause block; multi-line keeps its own layout), idempotent. The
LSP walks the new node without crashing; `${param}` uses ride the existing
`raw_param_refs` collector (find-refs lists them at the raw block; rename leaves the
opaque text alone — the miss is a loud E0113, same stance as D90). The tmLanguage
`#raw` rule is position-independent, so body-position raw already highlights.

**Not demonstrated in the helpdesk** — its workload report deliberately uses raw
value leaves over an engine-owned row set (D89 called that the better raw.md story);
forcing a raw body in would weaken the example. The live proof is the SQLite
integration test (raw body + bound param + `{table}` + hand-written tombstone filter
→ shape-typed rows; scalar raw `get`).

**Verified:** parser unit + golden (`raw_query`), sema unit ×9 + golden (clean /
E0210 both forms / E0113 / E0214 / E0212 / E0211-vs-unscoped-vs-E0182 / E0213 /
W0102 ×2 incl. the mention scan), codegen dml ×3 dialects + `;`-normalization,
client + openapi surface tests, fmt canonical/idempotent (single- + multi-line), LSP
inertness/rename test, live SQLite end-to-end; full `make check` green.

## D95 — guard re-entry: id minting is `&self`, the engine holds no lock across dispatch (NF3)

The D89-filed deadlock: `Engine::call*` locked the id generator (a `tokio::sync::Mutex`
around `Box<dyn IdGen>`, whose `next_id(&mut self)` forced exclusive access) for the
whole of `dispatch`. Guards run *inside* dispatch, so a guard calling the typed client
over its own engine re-locked the same mutex in the same task and hung — auth.md had to
carry a "use a second engine" caveat.

**Fix — remove the lock class, don't narrow it.** Minting is a sync, await-free
operation, so the synchronization belongs inside the generator, not around dispatch:
`IdGen` becomes `Send + Sync` with `next_id(&self)`. `SeqIdGen` counts on an
`AtomicU64` (same ids in call order); `UuidGen` is stateless. The engine stores a bare
`Box<dyn IdGen>` — no mutex at all — and `dispatch`/`run_mutation`/`plan_mutation`
take `&dyn IdGen`. With nothing to hold, the held-across-await shape is
*unrepresentable*: a regression can't be reintroduced without re-adding a lock. An
implementor with real shared state (a hi-lo block allocator) synchronizes internally —
a short std `Mutex` never held across an await, which is exactly the narrow critical
section the old design should have had.

Re-entry is also pool-safe, not just lock-safe: dispatch runs a mutation's guard
*before* checking out its connection, so a guard's own engine calls never contend with
a checkout the outer request already holds.

**Cancel-safety (D84 I2):** strictly improved — there is no lock to strand or poison on
cancel; the atomic can at worst skip a sequence number.

**Lock audit:** the id-gen mutex was the only held-across-dispatch lock. `MemStore`'s
internal std `Mutex` is taken per store operation, never across an await — the correct
shape, unchanged.

**Docs:** auth.md's second-engine caveat un-written — a guard may read through a
captured pool *or* call the typed client back over its own engine. The helpdesk guard
keeps its captured-pool read (a legitimate pattern; it never used a second engine).

**Proven:** an embed test where the registered guard calls `order_by_id` over the very
engine dispatching the guarded mutation and the write completes (5s timeout so a
regression fails fast, never hangs the suite), plus the full existing suites; `make
check` green end-to-end.

## D96 — raw marker renamed `sql` → `raw` (NF14)

The backtick escape hatch was spelled ``sql`…` `` while the feature is named **raw**
everywhere else — raw.md ("raw at the leaves"), principle 6's greppable-hatch rule, and
the `.mig` step form `raw(dialect)`. Owner call: one spelling, `raw`, across both
surfaces.

**Change.** grammar.ebnf `raw_sql = 'raw' '`' … '`'`; parser `is_raw_start` matches
`raw` (the lexer is untouched — the marker was always a contextual `LowerIdent`, and
`raw` stays contextual per D8, so `raw` remains legal as a field/param name); fmt
prints `raw` + backtick body; LSP keyword completion and the tmLanguage keyword list
offer/scope `raw` exactly as they did `sql` (same `keyword.control.bsl` scope — no
highlighting regression); parser error text names the `raw` body form.

**No back-compat alias.** Pre-1.0, all in-repo call sites migrate in the same change;
``sql` `` is now an ordinary parse error (unknown statement/value token), not a
special-cased hint. The greppability guarantee carries over unchanged: ``raw` `` is
the single greppable inventory of every site where guarantees stop, and it now also
greps *as* the feature name.

**Sweep.** Spec examples (raw.md, queries.md, soft-delete.md, grammar.ebnf comments),
conformance goldens (`raw_query` both suites), the helpdesk schema's raw leaves
(`ticket/model.bsl`, `ticket/queries.bsl`, `time_entry/queries.bsl`), and every test
fixture embedding `.bsl` source (parser/sema/codegen/fmt/LSP/runtime-live). Internal
Rust names (`RawSql`, `raw_sql`, `Tok::RawSql`) already said raw and are unchanged.
Historical decision entries keep the old spelling — they record what was decided then.

**Verified:** full workspace suites + conformance + fmt idempotence over the migrated
schemas + helpdesk live smoke; `make check` green end-to-end.

## D97 — `Page<T>` carries `with count`'s total (NF5)

The D89-filed drop: the wire already served `total` for a `with count` page (the second
COUNT statement, D56-era), but the generated `Page<T>` had no field for it — so
the helpdesk's `/admin/tickets` decoded rows + cursor and silently discarded the total
its own query paid for.

**Fix — one optional field, populated exactly when declared.** `Page<T>` gains
`total: Option<i64>`: `Some` exactly when the query declares `with count` (the wire
carries the field only then; serde's `Option` handling decodes an absent field to
`None`, so one shared envelope struct serves counted and uncounted pages alike). The
field is `#[serde(skip_serializing_if = "Option::is_none")]`, so a consumer re-serving
a typed `Page` (the helpdesk route does exactly that) mirrors the engine wire — no
phantom `"total": null` on an uncounted page.

**OpenAPI is per-query honest.** `page_schema` takes the query's `with count` flag
(read off the AST `Clause::Page`, the same derivation as the request-body page
controls): a counted query's inlined page schema advertises
`total: { type: integer, format: int64 }`; an uncounted query's schema doesn't carry
the property at all. `total` stays out of `required`, matching `cursor`'s treatment.

**Runtime untouched.** `shape()` already emitted `total` exactly for
`Envelope::Page { with_count: true }` (unit-proven in `tests/query.rs`); no gap found.

**Regenerated consumers.** The embed-gate mirror
(`based-runtime/tests/support/embedded_client.rs`, its schema gaining a keyset and a
counted offset page query) and all four checked-in example clients (three quickstarts +
the helpdesk). The helpdesk route needed no code change — re-serializing the typed
`Page` now serves the total, which is the user-visible payoff.

**Verified:** client + OpenAPI codegen tests (counted vs uncounted schema), typed
round-trip embed tests (`total: Some(57)` through the generated client; `None` and no
cursor on an uncounted short page), a live-SQLite end-to-end test (counted page →
`total: 5` beside a 2-row window; uncounted envelope has no `total` key), and the
helpdesk smoke extended to assert both admin pages serve the full live-set total;
`make check` green end-to-end.

## D98 — `-> ok`: the shapeless acknowledgement of a destructive mutation (NF4)

The D89-filed defect: a real DELETE (plain-model `delete` / `hard delete`) returns `{}` on the
wire — there is no surviving row to re-select (D58) — but the grammar's mandatory `-> ret_type`
forced a shape onto the signature, so the generated method's return type could never decode.
The helpdesk's `purge_comment` was declared but unrouted precisely because it was uncallable.

**Owner decision (2026-07-20): the shapeless ack return form, spelled `-> ok`.** A destructive
mutation declares `ok` instead of a shape: `mutation purge_comment(id: Id) -> ok scoped Tenant {
hard delete Comment where (id = $id); }`. `ok` is contextual (return-type position only, like
`stream`/`full`), so fields/models named `ok` are untouched; `ok[]` and `stream ok` don't parse.
Rejected alternatives:
- *Re-select before the delete* — reorders the write pipeline, reads a row the caller asked to
  destroy, and buys little for what is already the loud destructive opt-out.
- *Both forms legal* (shape = pre-delete snapshot, `ok` = ack) — two ways to say one thing; the
  snapshot semantics differ subtly from every other read-back (pre- vs post-write).
- *Sema-reject only* (no new return form) — leaves real DELETEs inexpressible entirely.

**One way to say each thing (principle 5, conservative call):**
- Shape + real DELETE is **E0220** (`shape-on-real-delete`): no surviving row on the return model
  → the declared shape could never decode. A tx sibling that creates/updates the return model
  keeps the shape legal (a surviving row exists).
- `-> ok` + any surviving write (create/update/restore/soft `delete`) is **E0221**: a surviving
  write's declared-shape read-back is the feature (D12/D58) — forfeiting it silently would make
  the safe contract optional. A **raw** write may ride along (its effect is outside the engine's
  knowledge — rejecting it would leave raw-write mutations with no legal return), but at least
  one real DELETE is required. The **first real DELETE's model is the primary model**: sema's
  `RMutation::ret_model` (scope ack, shard key, param typing) and the 404 check ride on it.
- `-> ok` on a query is **E0222** (parses, rejected loudly in sema with the fix named).

**Zero-row DELETE → 404 `not_found` (D92 extended to the destructive side).** The plan records
the primary DELETE's statement index (`MutationPlan::ack_check`); `apply_once` reads its
rows-affected — zero means the row was absent or out of scope: the transaction rolls back
(sibling deletes never survive), `RunError::NotFound` → wire 404, stable `not_found`, identical
for absent vs cross-tenant (no existence leak), and the idempotency claim is released (nothing
written, a retry may run). Secondary deletes in a tx may legitimately affect zero rows (childless
parent) — only the primary decides. Drivers already returned rows-affected; `MockDb` gained an
`affecting(rows)` knob.

**Generated surface.** Codegen already emitted no re-select for a real DELETE (D58) — unchanged.
Client: an ack mutation's method (and its `_with_key` twin) returns `Result<(), ClientError>`,
decoding the wire `{}` through a shared `#[derive]`d empty `Ack` struct emitted only when the
schema declares an ack mutation (a schema without one is byte-identical, embed-gate-verified).
OpenAPI: the `200` is a shared empty-object `Ack` component (`additionalProperties: false`),
registered under the same gate. fmt prints the bare `ok`; tmLanguage scopes `ok` after `->` like
the other return-position keywords; the LSP skips `ok` as a type reference (it is a token, not a
name — hover shows the signature as written).

**Helpdesk (the motivating symptom).** `purge_comment` is now `-> ok`, the client regenerated,
and the route wired: `DELETE /admin/comments/{id}` (agent-gated) → `200` bare ack; the smoke
drives requester → 403, cross-tenant purge → 404 (row untouched), purge → 200, re-purge → 404.

**Verified:** parser unit + goldens (`ack_mutation`, `ack_ret_no_brackets`), sema unit + goldens
(`ack_delete`, `ack_errors` — E0220/E0221/E0222, scope ack still owed, tx-sibling survival),
codegen unit (no re-select; unit-returning client + `Ack` gating; OpenAPI component + `$ref`),
runtime unit (ack commit → `{}`; zero-row → NotFound + rollback), typed live-SQLite embed proof
(purge → `Ok(())`, row gone, re-purge → typed 404 `not_found`), and the extended helpdesk smoke;
`make check` green end-to-end.

## D99 — the guard re-entry handle is first-class: `GuardRequest::engine()` (NF3 follow-up)

D95 proved a guard *can* call the typed client over its own engine, but the seam to do
it was missing: the engine is constructed *after* its guards are registered, so a guard
closure had nothing to capture. The re-entry test wired the engine in after the fact via
an `Arc<OnceLock<Arc<Engine>>>`, and the helpdesk guard sidestepped the whole thing by
reading through a captured sqlx pool — hand-writing the workspace scope (`org_id = $2`)
and the tombstone filter (`deleted_at is null`) that the schema already declares. That is
exactly the class of hand-written filter the language exists to make unforgettable: the
guard is "just doing SQL" against a model the engine owns, so the engine should own it.

**The handle rides the request.** `GuardRequest` gains `pub(crate) engine: Option<Engine>`
and a `pub fn engine(&self) -> &Engine` accessor; dispatch passes the dispatching engine
through to `check_guard`, which fills it in. A guard's state-reading decision becomes
`client::embedded(req.engine()).ticket(input, ctx).await` — the schema's own scoped,
soft-deleted query, deserializing `req.args`/`req.ctx` straight into the generated input
and `$ctx` types. No filter is restated; the read can't drift from the model's contract.

- **`Engine` is now a `Clone` handle** over an inner `Arc<EngineInner>` (like a pool
  handle), so passing `Some(self)` into dispatch and cloning it into `GuardRequest` is
  cheap and the app can hold the engine directly instead of `Arc<Engine>`. No behavior
  change: every clone runs the same engine, connection concurrency is still the backend
  pool's.
- **`engine()` is infallible in practice.** It is `Some` on every `Engine::call`; the
  only `None` path is a raw `dispatch` (a test/edge harness), where a guarded schema is
  never served anyway — the standalone HTTP listener refuses a guarded schema at startup
  (D88), so it also passes `None`. The accessor panics on the `None` path with a message
  naming the cause; a production guard always has the handle.
- **Safety is unchanged from D95.** Dispatch holds no engine-wide lock and a guard runs
  before its mutation checks out a connection, so re-entry neither deadlocks nor starves
  the pool — now the *default* path, not a documented-but-awkward capability.

**Docs.** auth.md's registration bullet leads with the re-entry handle (read through the
schema's queries; captured pool is the fallback for state the schema doesn't model);
guard.rs mirrors it. D95's captured-pool note is superseded here.

**Helpdesk.** `caller_can_close` drops sqlx entirely — no captured pool, no raw `select
status … where … deleted_at is null` — and reads through `ticket`, inheriting scope +
soft-delete. The app stores `Engine` (not `Arc<Engine>`); the smoke's requester→403 /
cross-tenant→404 / resolved→200 close path is unchanged (the injected filters now come
from the schema, so the cross-tenant denial is the scope's, not a hand-written `org_id`).

**Verified:** the re-entry unit test rewritten to `req.engine()` (no `OnceLock`) stays
green under its 5s deadlock timeout; the full guard suite (allow/deny/unregistered/
build-refusal) green; the helpdesk builds and its smoke path is unchanged; `make check`
green end-to-end.

## D100 — atomic update expressions: a self-referential arithmetic SET (T3)

**Decision.** An `update` assignment's right-hand side may be a scalar **arithmetic
expression** over the target model's own numeric columns, `$param`s, and numeric literals —
`update Product where (id = $id) { qty = qty + $delta }` — lowered to a real SQL
`SET qty = (qty + ?)`, computed in the database, never a read-modify-write. This closes the
lost-update gap (two concurrent adjustments compose off the stored value) and is the T3
item on Track T (core DB feature parity, after enum D82 and decimal/float D83).

**Scope (minimal, principle 5 — no Turing-creep).** Operators are `+ - * /`; `*`/`/` bind
tighter than `+`/`-`, left-associative, parenthesize to override. Operands are numeric
columns of the updated model (a bare name = the row's pre-write value), `$param`s, and
numeric literals. No functions, conditionals, or cross-row references — the expression is a
leaf-level escape into arithmetic, not a language. A plain value stays the one form for
`create`.

**Eligibility.** The numeric family (`int`/`float`/`decimal`, D83) end to end: every column
operand must be numeric (`E0231`) and the assigned column must be numeric (`E0153`, the
ordinary assign-type rule — a numeric expression assigned to a text column is that error).
An arithmetic RHS is **update-only** — a `create` has no existing row to self-reference
(`E0230`). Params and functions are typed at their declaration / unmodelled, so they are
skipped by the numeric check, exactly as on every other write-side family check.

**Grammar / AST.** `assign = column_name '=' assign_rhs`, where `assign_rhs` is a
precedence-climbing arithmetic expression whose leaves are the existing `value` production
(a bare `value` is the common, and for `create` the only, case). New lexer tokens `+ - * /`
(the `->` arrow still wins the longest match, so `-` is unambiguous). AST: `Assign.value` is
now `AssignRhs` (`Value(Value)` | `Arith { lhs, op, rhs, span }`) with `ArithOp`; an
`as_value()` accessor lets the many single-value sites keep their `Value` logic.

**Lowering.** A column operand reads through the ordinary value path (qualified `table.col`,
which all three dialects accept on a SET RHS — SQLite included, verified); each binary node
wraps in parens so the SQL evaluates in AST order. The SET target keeps its dialect form
(MariaDB qualifies, Postgres/SQLite bare — D58). Division is the database's (integer vs.
real per operand types); no zero-guard. A `$param` operand binds positionally at the target
column's family (so Postgres numeric text-binding, D59, still holds).

**Editor / fmt.** fmt reprints the RHS with minimal parentheses (idempotent, reparses).
Field-reference go-to-def / find-refs / rename see every RHS column operand (rooted at the
update target) and every `$param`. No new keywords, so tmLanguage/completion are untouched.

**Verified.** Sema positive/negative cases (E0230, E0231, E0153) + a conformance-sema
golden; codegen SQL asserted on all three dialects (`SET \`product\`.\`qty\` = (…)` /
`SET "qty" = ("product"."qty" + :delta)`); fmt round-trip; and **live SQLite** read-your-
writes proving the sum is computed server-side and two sequential adjustments compose
(100 + 25 → 125, 125 − 5 → 120). `make check` green end-to-end.

## D101 — aggregations + group by + having (T4)

**Decision.** A shape may project **aggregates** — `count()`, `sum(col)`, `avg(col)`,
`min(col)`, `max(col)` — as `= ` values (`orders = count()`, `revenue = sum(total)`); a
shape carrying any is an *aggregate shape*, a projection over **groups** rather than rows. A
query pairs it with `group by (cols)` and `having (pred)`. This is the T4 item on Track T
(core DB feature parity, after enum D82, decimal/float D83, atomic update exprs D100).

**Surface (readable > terse; one way to say a thing).** Aggregates live in shapes because
that is where projections live; `group by`/`having` live on the query because grouping is an
engine instruction (the same contract/implementation split as `-> Shape[]` vs `list`). New
AST: `ShapeValue::Agg(AggCall { func, arg })` and `Clause::GroupBy(Vec<Path>)` /
`Clause::Having(Predicate)`. `count()` is arg-less (rows in the group); the other four take
one column. The function set is closed in sema (`KNOWN_AGGS`), so the grammar's open
`agg_func` degrades to `E0240` on anything else. `having` reuses the ordinary predicate
grammar untouched — its left operands are the shape's **projected names** (an aggregate alias
or a group column), so no aggregate node pollutes the shared predicate language; `order` in
an aggregate query likewise names projected columns.

**Reconciled with the existing `count`.** Pagination's `with count` (NF5/D97) stays the
`Page<T>.total` row-count metadata; T4's `count()` is a projected aggregate column. Different
positions, no conflicting spelling — the aggregate-query path never paginates, so they never
meet.

**Type rules.** `count()` → `int` (non-null). `sum`/`avg` need the numeric family (D83:
int/float/decimal); `min`/`max` need a *comparable* column (numeric/timestamp/date/text);
an enum or relation operand is never eligible (`E0241`). Results: `sum` keeps the column's
numeric type, `avg` is always `float`, `min`/`max` keep the column's type — and all four are
**nullable** (an empty or all-null group aggregates to null), so the client/OpenAPI type is
`Option<T>` for them and a bare `i64`/integer for `count`.

**Group-by consistency, enforced (not deferred to the DB).** Every non-aggregate projected
column must be a `group by` column (`E0242`); with no `group by` the shape must be
all-aggregate — one whole-table row (a `get`, exempt from the unique-key rule). `group by` /
`having` are legal **only** on an aggregate query (`E0243`). An aggregate query cannot be
paginated (`E0244` — grouped keyset paging is deferred) and takes no default model `@sort`
(an ungrouped sort key isn't a valid grouped column). An aggregate shape is **flat** and
never nested, referenced, or a mutation return (`E0245`).

**Composition kept correct.** `where` (row filter), soft-delete, and `@scope` all inject into
the `WHERE` — narrowing rows *before* grouping — so a scoped/soft-deleting model aggregates
only its live, in-scope rows, and the scope column need not be grouped. `having` filters
groups *after* (HAVING vs WHERE lowering kept distinct).

**Codegen — deterministic decode via dialect casts (runtime unchanged).** An aggregate query
lowers to its own `SELECT … GROUP BY … HAVING … ORDER BY …` (no keyset/LIMIT/count query).
Because drivers decode by the DB's *returned* column type, each aggregate is cast so the wire
shape is fixed: `count` → the dialect integer (already `int8`/`INTEGER` everywhere, no cast);
`sum(int)` cast back to an integer where the dialect widens it (Postgres `SUM(bigint)`→numeric,
MariaDB→decimal — `CAST(… AS BIGINT/SIGNED)`; SQLite keeps it int); `sum(decimal)` stays the
native `NUMERIC`/`DECIMAL` (exact-string decode) except SQLite, where decimal is `TEXT` and the
sum is float-degraded, cast to `TEXT` for the string wire form; `avg` cast to the dialect double
(`DOUBLE`/`DOUBLE PRECISION`/`REAL`); `min`/`max` keep the native type. `HAVING`/`ORDER BY`
inline the **numeric** aggregate (not the SELECT alias — Postgres forbids it — and not the
decimal-to-text cast, which would break an ordered comparison). So only codegen changed; the
plan/scan/value/decode paths were untouched.

**SQLite decimal is degraded (documented, per D83).** `sum`/`avg` over a `decimal` on SQLite
compute through float (TEXT affinity), and `max`/`min` compare lexicographically — production
dialects (`DECIMAL`/`NUMERIC`) are exact. The live proof therefore asserts exact values on int
columns and the honest float-degraded value for the decimal sum.

**Editor / fmt.** fmt reprints `= count()` / `= sum(col)` and `group by (…)` / `having (…)`
canonically (round-trip stable). `group`/`by`/`having` join the keyword vocabulary
(tmLanguage + completion) and the aggregate function names complete as functions; group-by
columns and aggregate-argument columns are go-to-def/find-refs/rename sites (rooted at the
query target / shape `from`), matching the D90 field-reference precedent — a `having` operand
names a shape-local alias, so it is not a model-field reference.

**Diagnostics:** `E0240` (unknown aggregate or wrong argument arity), `E0241` (ineligible
aggregated column), `E0242` (a non-aggregate projected column not grouped, or an `order`/
`having` name not projected), `E0243` (`group by`/`having` without an aggregate shape),
`E0244` (`page` on an aggregate query), `E0245` (aggregate-shape composition — nested,
referenced, or a mutation return).

**Verified.** Sema +/− (E0240–E0245 + the group-by-consistency and global-aggregate-get
cases); a sema conformance golden; codegen SQL asserted on all three dialects (the cast
matrix — `COUNT(*)`, `CAST(SUM(int) AS SIGNED/BIGINT)`, native/`TEXT` decimal sum, `CAST(AVG
… AS DOUBLE/…)`, inlined `HAVING`/`ORDER BY`); client field typing (`i64` / `Option<Decimal>`
/ `Option<i64>` / `Option<f64>`); fmt round-trip; and **live SQLite** — a real `GROUP BY` /
`HAVING` query grouping per buyer, excluding a soft-deleted row before grouping, filtering
groups, and ordering them, with `count`/`sum`/`avg`/`max` decoded to their wire types.
`make check` green end-to-end. Spec: `spec/syntax/shapes.md` + `spec/syntax/queries.md`.

## D102 — many-to-many + upsert (T5)

**Decision.** Two independent T5 features. **(A) Many-to-many** is modeled by an **explicit
junction model** — a model with a forward edge to each side plus a to-many inverse on each end
(`Enrollment { student: Student, course: Course }`, `Student.enrollments`, `Course.enrollments`)
— so m2m needs no new relation syntax: it is the existing forward+inverse machinery, and a shape
reaches the far side through the junction with the existing to-many nesting (D57). **(B) Upsert**
is `create <Model> { … } on conflict (target) update { … }`: on a unique-key collision the
`update` branch runs over the existing row instead of inserting. This is the T5 item on Track T
(after enum D82, decimal/float D83, atomic update D100, aggregations D101).

**This iteration ships (B) upsert fully; (A) m2m is specified, and the explicit-junction pattern
already works** (it is forward+inverse relations, proven by L1/D57). The **far-side flattening
projection** (`courses = enrollments.course { … }` → a flat `Vec<Course>` skipping the junction)
and any **implicit-junction sugar** (`courses: Course[] <-> students`) are the deferred next T5
slice — held on principle: an engine-generated join table is real, write/disk-costing DDL a
reviewer must see in the PR (principle 2 / the NF11 tension the owner is weighing for inferred
indexes), so it wants explicit-in-source resolution, not a silent default. Spec: relations.md.

**Upsert surface (mutations.md).** `on conflict (col[, col]) update { assigns }` is an optional
tail of `create` (AST `WriteStmt::Create.conflict: Option<OnConflict>`; grammar `on_conflict`).
The conflict target names a unique key; the `update` branch is an ordinary update assign block —
plain values + the same self-referential **arithmetic** D100 gives an `update`, so
`on conflict update { hits = hits + 1 }` composes on the **stored** value (the canonical
counter/accumulate use). One way to say a thing: `on conflict` is the only upsert spelling;
`create`/`update` are otherwise unchanged.

**Validation (safe by default; five stable codes).** `E0250` conflict target is not a declared
unique key (a `(unique)` column, a `@index (…) unique` matching the set, or the pk — a conflict
needs a key the DB enforces). `E0251` the `update` branch assigns a conflict column (moving the
key breaks the conflict + the read-back). `E0252` a conflict column is neither set by the create
nor scope-managed (no value to conflict on / read back by). `E0253` `on conflict` on a
`@soft_delete` model (a tombstoned row still holds its unique key — an upsert would silently
update the tombstone, not insert; delete-aware upsert is a separate, explicit feature). `E0254`
a scoped model's conflict target omits a scope column — else a conflict could match, and the
update silently modify, **another scope's row**; requiring the scope column in the key confines a
conflict to the caller's own scope (an `unscoped` mutation forfeits this like every other scope
guarantee). The update branch is otherwise checked like an ordinary update (E0153 type agreement,
E0231 arith numeric).

**Lowering (per-dialect over the `Dialect` seam).** Postgres/SQLite emit `INSERT … VALUES (…)
ON CONFLICT (cols) DO UPDATE SET …`; MariaDB `INSERT … ON DUPLICATE KEY UPDATE …` — its form
carries **no explicit conflict-target list**, so the validated key's uniqueness is what makes the
two agree. The conflict-`SET` columns render **bare** on both sides on every dialect (a bare RHS
column names the existing row; a qualified one is rejected/ambiguous in the conflict clause) — a
`Select::with_bare_cols` mode reusing the ordinary assign lowering (enum variants → wire literals,
the D100 arithmetic). `@scope` auto-set on the insert is unchanged.

**Read-back keyed on the conflict target (not the id).** A conflict path keeps the **existing
row's** id, so the INSERT's generated id would miss it. The declared-shape re-select therefore
keys on the conflict target's inserted value (`RetKey::Conflict`, built in codegen from the
create's own value for each target column — a `:param` or the `:ctx_<field>` scope auto-set),
plus the scope/live guards a `get` applies. The runtime is **unchanged**: the re-select's
placeholders are params/`$ctx` already in the bind environment (the create-keyed `:result_id` is
still seeded but unused here). So a plain create keys on its id (D12), an update/soft-delete/
restore on its `where` (D58), an upsert on its conflict target — three keys, one re-select path.

**Editor / fmt / client.** fmt reprints `create … on conflict (…) update { … }` canonically
(round-trip stable). The conflict-branch assigns are ordinary write-body references — go-to-def/
find-refs/rename (rooted at the create model), enum-variant nav, and `$ctx`/param collection all
walk them, so a param used only in the branch still types the client method and joins the bag.
No new client/OpenAPI surface — an upsert returns its declared shape exactly like any create.

**Verified.** Parser round-trip (conflict target + update branch); sema +/− for E0250-E0254 + two
clean cases (single `(unique)`, composite `@index unique` with a scoped model) + a conformance
golden; codegen SQL asserted on all three dialects (MariaDB `ON DUPLICATE KEY UPDATE`, Postgres/
SQLite `ON CONFLICT (…) DO UPDATE`, the conflict-keyed re-select, composite+scoped target); fmt
round-trip; and **live SQLite** — insert path then repeated conflict paths compose on the stored
value (`hits` 1→2→3→4), a second key is an independent counter, read-your-writes on both paths.
`make check` green end-to-end. Spec: `spec/syntax/mutations.md` (upsert) + `spec/syntax/relations.md`
(m2m) + `spec/grammar.ebnf`.

---

The next five decisions (D103–D107) resolve the owner-flagged design follow-ups NF11/NF9/NF7/NF8/
NF13 in one pass (owner-approved each fork, 2026-07-21). They are **decided, not yet implemented** —
each states the target and its spec seam so a build-loop iteration lands it mechanically. D103 is the
keystone (it changes a principle); the others reference it.

## D103 — inferred indexes + implicit `id` become explicit-in-source (NF11)

**Decision.** Stop silently deriving structure. The two engine-created facts that carry
independent, PR-invisible cost — a join-key **index** and a model's **primary key** — move from
silent derivation to **written in source, enforced by a compiler error with a one-key LSP
autofix.** (1) A relation join key some query/shape traverses with no covering `@index` is a new
error **`E0260`** (promoted from the `W0103` lint) — satisfied by `@index <field>` (the autofix
inserts it) or the existing visible `unindexed(max_rows: N)` / `unindexed(unsafe)` opt-out.
(2) A model that declares no `id` is a new error **`E0261`** — the autofix inserts the `id` line.
Both fire in `based check` (CLI), not only the editor, so the compiler is equally honest headless.

**Why — principle 8 is reworded.** Principle 8 (“show, don’t write, for derived facts”) currently
*names inferred indexes as its example*; this decision inverts that example. An index has real
write + disk cost and a PK is load-bearing, so both are *consequential* — principle 2 (“nothing
consequential is true by omission”) governs: their omission is neither single-meaning nor free, so
they are not elidable, and hard priority 3 (a reviewer confirms design by reading the PR) is the
clincher — an editor-only inferred fact never reaches the PR. New principle 8: *“Show, don’t write —
only for cost-free, unambiguous derived facts (an inverse name, fixed by the written forward edge).
A derived fact a reviewer must weigh — an index, a primary key — is written in source; the engine
errors when it’s missing and offers a one-key autofix.”*

**What retires.** `IndexSnap.inferred` (`migrate/model.rs`), the `inf_` DDL naming +
soft-delete-prepend baseline in `sql.rs:88`, `RModel.inferred_indexes` + the `indexes.rs` baseline
build, and the `FactKind::InferredIndex` fact + its LSP inlay (`based-facts` + `compile.rs:605`) all
go. The DDL’s indexes become exactly the written set. `W0104`/`W0105` (useless / stale-annotation)
stay — they lint *declared* `@index`. **Kept:** when a user writes `@index <field>` on a
`@soft_delete` model, the engine still renders it soft-delete-leading (predicate-equivalent); that
is a *rendering* of the written index, not a second silent index, and stays (document in
indexing.md). **Kept:** the inferred *inverse pairing* stays a shown fact (`FactKind::InferredInverse`)
— the inverse field is written in source and only the unambiguous pairing is derived, which passes
principle 2’s elision test (one meaning, safe, visible). NF10/D91’s narrow fact anchoring stands.

**Consequence — resolves D102’s deferred m2m fork.** A junction model’s two FK columns each need an
explicit `@index` (no silent join-table index), and there is **no implicit-junction sugar** (it
would emit silent DDL, exactly what this decision forbids). So m2m stays the explicit-junction
pattern; only the far-side flattening projection (`courses = enrollments.course { … }`) remains as
sugar over existing machinery — approved, queued, no silent-DDL concern.

**Spec seam.** principles.md (P8, reworded now), models.md (Defaults: `id` now required; Types),
indexing.md (Inference → “what you must index”), D2/D15 revised-by-this-entry. Fallout on landing:
conformance goldens + `spec/examples/commerce` + the four `examples/*` schemas gain explicit
`@index`/`id` lines; the migration goldens lose the `inf_` indexes. Codes `E0260`/`E0261`.

**Implementation note.** `E0260` subsumes the whole retired `W0103` check, not only the
join-key case: it fires both on a traversed join key with no covering `@index` *and* on a query
whose root eq/range/leading-sort filter no index leads with (both "the query will scan"), each
satisfied by an `@index` or the `unindexed(…)` opt-out and each carrying the autofix. The
one-key fix rides on the diagnostic as a `Fix{model, line}` (based-diagnostics) that the LSP's
`code_action` handler turns into a body-insertion edit. Codegen/runtime *lowering* tests
(dml/mutations/openapi/client-adjacent + the runtime suites) tolerate `E0260` in their clean-schema
guard — they exercise SQL/client emission, not index completeness, which is covered by based-sema's
tests + the conformance goldens + the (indexed) examples.

**`@no_id("reason")` — the E0261 opt-out for keyless legacy tables (owner, 2026-07-22).** A legacy
DB being adopted may have a table with *no primary key at all*; E0261 would wall it out. `@no_id("reason")`
is the escape hatch: presence suppresses E0261, and the model carries no synthesized `id`. The **reason
string is mandatory** (like `unscoped("reason")`) — an empty/missing one is `E0262` — so a forfeited key
is never silent in review. (A legacy PK with a different name/type/column is *not* this case — declare it in
the `id` slot, `id: Id (column "account_pk")`.) A keyless model forfeits its id-keyed operations, each
enforced with a loud compile error rather than a silent miscompile: get-by-id is impossible (a `get` keys on
a `(unique)` column, else `E0144`); a keyset `page` has no `id` tiebreaker so its sort must carry a unique
key (`E0263`, or `page … offset`); a declared-shape `create` has no generated id to read back by, so it must
set a `(unique)` column the re-select keys on (`E0264`, or `-> ok`) — reusing the upsert's conflict-key
re-select machinery, so the runtime binds it with no change; and a forward relation *to* a keyless model is
`E0265` (no `id` to reference). Codegen drops the `PRIMARY KEY`/`id` column (DDL + snapshot, via a `no_id`
flag on `RModel`/`TableSnap`) and the keyset `id` tiebreaker. **`@no_id` is a deliberate exception to the
positive-framing convention** — it is a factual schema descriptor (this table has no id), not
define-by-negation prose, so a later framing pass must not "correct" it. Spelling is snake_case to match every
other decorator (not `@keyless` — a keyless table may still carry foreign keys; what is absent is
specifically the `id`). Codes `E0262`–`E0265`.

## D104 — opaque column + index passthrough via `raw(…)` (NF9)

**Decision.** The closed primitive set (`text int bool timestamp date json uuid float decimal`) gets
a single escape hatch so a DB type the engine doesn’t model (PostGIS `geometry`, `tsvector`, `inet`,
vendor JSON variants) no longer forces a raw migration behind the schema’s back — which today makes
the snapshot blind on a modeled table, gets the column silently dropped by a sqlite table-rebuild,
and excludes it from every generated surface (the “throw the whole system away for one field” cliff).
The hatch is the existing **`raw` keyword (D96)**, now valid in **type** and **index** position —
“raw at the leaves, never the structure” (principle 6). *(Spelling: `raw`, not a new word. Standing
convention, owner 2026-07-21: **never use `sql` as a keyword/marker anywhere in the language** — we
are Postgres-compatible and `sql` reads as ambiguous; this is why D96 renamed `sql`→`raw`, and it
binds all future syntax.)*

- **Opaque column type.** `location: raw("geometry(Point,4326)")?` — the engine stores the literal
  type string in DDL + the neutral snapshot, so diff = string compare and migrations / sqlite
  rebuilds / `@was` all keep working. A **per-dialect map** when the type name differs:
  `tags: raw({ postgres: "tsvector", mariadb: "text" })`; a bare string applies to all targets. A
  dialect that is a compile target but absent from the map is **`E0270`**. The value is **opaque end
  to end** (Prisma’s `Unsupported` rule): the client treats it as an opaque string, it is **excluded
  from `create`/`update` unless nullable or defaulted** (**`E0273`** on assigning a non-nullable,
  non-defaulted opaque column — you can’t construct a value the engine doesn’t model), and
  `where`/`order`/aggregate on it is **`E0271`** — *except* through the existing `raw` predicate /
  `ShapeValue::Raw` leaf (D96), which already handles the read side (`ST_Area(location)` is a raw
  shape value today). One opaque field degrades gracefully; CRUD on the rest of the model,
  migrations, and the drift check all stay in-system.

- **Exotic indexes — two tiers, same seam** (an opaque column you can’t index is dead weight — a
  `geometry` without GIST, a `tsvector` without GIN, is unusable). (i) `@index(location) using gist`
  — a `using <method>` token (gist/gin/brin/hash…; MariaDB `fulltext`/`spatial`), snapshot-recorded,
  per-dialect validity checked **loudly** at gen (**`E0272`** — sqlite lacks most methods; an error,
  never a silent skip). (ii) `@index raw("(lower(email))")` — the long-tail opaque index (expression
  indexes, opclasses, partial `WHERE`) recorded as a literal string in the snapshot, diffed by string
  compare; an empty/unparseable raw index is **`E0274`**. `IndexSnap` grows `method: Option<String>`
  + `raw: Option<String>` (and drops `inferred` per D103), so create/drop/rebuild lifecycle stays
  in-system for exotic indexes exactly as for opaque columns.

**Spec seam.** models.md (Types: opaque type), indexing.md (`using` + opaque `@index raw`), raw.md
(the type/index positions of `raw`; the `sql`-is-banned convention), migrations.md (opaque diff =
string compare). Codes `E0270`–`E0274`.

**Shipped (2026-07-23).** Implemented end to end as decided. Notes on the as-built:

- **Syntax + AST.** `BaseType::Raw(RawSpec)` (a model-field-only base type; the parser rejects an
  opaque type in a param annotation, a scope term, or with a `[]` array suffix — an opaque value has
  no array form). `RawSpec` is `All(String)` or `PerDialect([{dialect, text}])` with `for_dialect`,
  `render` (source order), and `canonical` (dialect-sorted — the snapshot form, so map order never
  churns a diff). `IndexDecl` grows `method: Option<Ident>` + `raw: Option<RawSpec>`. `raw(…)` (parens)
  and ``raw`…` `` (backticks) share the keyword, disambiguated by the following token.
- **The two-phase check.** The dialect-free `check` catches everything target-independent (opaque
  operand `E0271`, opaque assign `E0273`, empty body `E0274`, unknown method / unknown map dialect).
  A new **`check_target(&schema, dialect)`** catches what only a compile target decides — a
  per-dialect map missing the target (`E0270`) and `using <method>` on a target lacking it (`E0272`,
  e.g. every method on sqlite). The CLI runs it with the manifest dialect; the LSP with the resolved
  project dialect (skipped for a loose file with no project). `Terminal::Opaque` threads the opaque
  marker through the shared resolver so filter/sort/group/aggregate all reject via one `reject_opaque`.
- **Index method map.** `btree`/`hash` → postgres+mariadb; `gist`/`spgist`/`gin`/`brin` → postgres;
  `fulltext`/`spatial` → mariadb; sqlite has none. Postgres renders the leading `USING <m>`; MariaDB
  spells `fulltext`/`spatial` as index *kinds* (`FULLTEXT KEY`) and `btree`/`hash` as a trailing
  `USING`. An opaque `@index raw("…")` always emits as a standalone `CREATE INDEX` (never a MariaDB
  inline `KEY`); its name is content-derived (`idx_<table>_raw_<hash>` over the canonical body), so
  reordering a model’s indexes never reads as a rename. Exotic + opaque indexes never satisfy `E0260`
  and never trip `W0104` (the engine can’t see the access path they serve — the author asserts it).
- **Live proof.** SQLite integration test: an opaque column + opaque index in the generated DDL,
  a `create` that omits the opaque column, and a read-back both bare (opaque string) and through a
  `raw` value leaf (`length(shape)`). Also unit (+/− sema, DDL all three dialects, snapshot
  round-trip + string-compare diff, client/openapi String), a conformance golden (`raw_opaque`), and
  a fmt round-trip.

## D105 — `@was` lifecycle: `gen` self-consumes + teach-at-checkpoint (NF7)

**Decision.** `@was("old")` is a one-shot gen-time rename hint; today it lingers after
`based migrate gen` as `W0107` cruft (a second commit to remove) or the author strips it and the
rename gesture never appears in any PR, so users never learn it — and a rename authored *without*
`@was` becomes a silent destructive drop+add with no “did you mean a rename?” anywhere. Three moves
(one already shipped):

1. **`gen` self-consumes the spent `@was`.** After `gen` writes a migration that consumed a
   field/model `@was`, it strips that exact `@was` token from the `.bsl` (minimal diff, only the
   spent token) and prints a visible line — e.g. *removed spent `@was("old")` from `Model.field`
   (recorded in migrations/NNNN_slug)*. The durable record is the `schema.snap` chain + `up.mig`’s
   `rename` step (principle 4: one source of truth — the rename lives in the ledger, not permanently
   in the model), so the source annotation is safe to retire automatically: no `W0107` cruft, no
   second commit, works headless. `gen` already writes `migrations/` and `based fmt` already rewrites
   `.bsl`, so a toolchain source edit is in-band; a discarded `gen` reverts source + migration
   together under git. `W0107` stays as the fallback lint for a hand-authored migration where `gen`
   didn’t run.

2. **Teach-at-checkpoint (the load-bearing piece).** When a single-table diff drops column X and adds
   a same-family column Y (one drop + one compatible add on one table), `gen` stdout, the `W0108`
   drift note, and the apply destructive-gate message all add: *“if this renames X→Y, add
   `@was("X")` on Y and re-run `based migrate gen`; otherwise X is dropped (data loss).”* This gives
   `@was` the interactive-prompt’s self-revealing-at-ambiguity property over the run→read→edit→re-run
   loop, with zero prior knowledge and no TTY dependence (so it works for headless agents; interactive
   `gen` prompts stay rejected — non-TTY hangs, non-reproducible gen). No new code — a hint on the
   existing destructive/drift detection.

3. **Editor rename inserts `@was` — already shipped (D80).** `textDocument/rename` on a field/model
   mapped to a live column/table inserts `@was("old")` as part of the rename edit.

Rejected: interactive gen prompts; keep-forever Terraform-`moved`-style hints (the ledger already
holds transition history). Spec seam: migrations.md E5.

**Shipped (2026-07-23).** `based-codegen::migrate::lifecycle` holds the two offline helpers on the
existing diff engine:
- `spent_was_edits(steps, schema, decls, sources) -> Vec<SpentWas>` keys off the `rename` steps the
  migration **actually emitted** (a spent/inert `@was` produces none), maps each back to its
  field/model `@was` via the schema's physical-column/table names, and returns the surgical byte range
  to remove. Field-level removal reconstructs the full `@was("…")` extent from the string-literal span
  the parser keeps (the only span it stores) by scanning out to `@was(` and `)`, then eats the single
  separating whitespace so `text? @was("x") (unique)` → `text? (unique)`. Model-level removal takes the
  full decorator span; when it sits alone on its line the whole line (incl newline) goes, else the
  minimal directive±one-space edit. `apply_spent_was` applies highest-offset-first (idempotent). The
  **ambiguity D105 didn't pin — a `@was` sharing a line with other decorators — is resolved to the
  least-surprising minimal edit** (directive + one adjacent space, decl untouched), noted in the source
  doc, not an owner-level policy.
- `rename_hints(prev, now) -> Vec<RenameHint>`: one hint per table with **exactly one** dropped column
  and **exactly one** added column of the **same neutral type family** (a 2-drop/2-add table is left
  silent — the pairing would be a guess, precisely what `@was` makes explicit). `RenameHint::message()`
  is the shared string.

Wiring: `based-cli` `cmd_migrate_gen` runs the self-consume (writes each touched `.bsl` back, logs each)
then prints the hints; `cmd_migrate_apply` prints the offending migration's hints at the
`MigrateError::Destructive` gate. `based-runtime` `PlannedMigration` gained `rename_hints` (computed in
`load_migrations` from the same per-migration `prev → snap` diff). `based-lsp` `drift_diagnostics`
appends the hint to the matching model's `W0108` note. `W0107` (spent-`@was`) is unchanged. Gate: full
`make check` green (fast gate + all three live suites + all examples + the axum-helpdesk smoke).

## D106 — `up.mig` is snapshot-authoritative: honest contract + a real editable surface (NF8)

**Decision.** The generated `up.mig` header says “edit if needed, then apply,” but apply/render
re-derive **structural** SQL from the `schema.snap` chain and only *parse* `raw(<dialect>)` lines out
of `up.mig` (`based-runtime` `load_migrations` → `migrate::diff_snapshots` for structural,
`parse_raw_steps` for raw) — so a hand-edit to a structural step line is **silently ignored at
apply** (only offline `based migrate verify` catches the byte-drift, and only if it runs). Six fixes:

(a) **Honest header.** `render_up`’s header (`migrate/up_mig.rs`) states the real contract:
structural steps derive from `schema.snap` (editing a structural line has no effect at apply); the
editable surface is `raw(dialect)` lines (which run *after* all structural steps, regardless of file
position) and a hand-authored `down.mig`.

(b) **Apply-time drift refusal (owner: refuse, hard error).** `load_migrations` already reads both
`up.mig` and `schema.snap` per migration, so the `verify` byte-compare is nearly free there:
apply/render **refuse** (new `MigrateError::UpMigDrift`, clear message) when a migration’s structural
`up.mig` lines diverge from the snapshot-derived SQL, instead of silently ignoring the edit — closing
the “verify-didn’t-run” hole (principle 1: a dangerous silent-ignore becomes an explicit stop at the
moment of harm). Cosmetic whitespace/comment edits are still tolerated (the compare canonicalizes
like `content_hash`).

(c) **`.mig` (and minimal `.snap`) editor support.** A `.mig` tmLanguage grammar + VS Code language
contribution (steps, `raw(dialect)`, the `# DESTRUCTIVE` marker, embedded SQL inside raw backticks);
`.snap` gets a language id so it isn’t plain text (generated → minimal). Same per-dialect SQL
treatment NF12 wants for `.bsl` raw — the `.mig` `raw(dialect)` token names its dialect explicitly.

(d) **Multi-line `raw` steps.** `parse_raw_steps` is line-based (single-line only), so the sqlite
table-rebuild the spec itself cites is unwritable readably. Fix: `raw(dialect)` may be followed by a
**backtick-delimited multi-line block**, keeping the one-file artifact + the `content_hash` tamper
contract (the whole `up.mig` is hashed, so multi-line raw stays covered). Sidecar files rejected
(they’d need `up_hash`/`verify` extended or post-apply edits dodge the tamper check).

(e) **`down.mig` placeholder.** `gen` emits `down.mig` prefilled with real reverse SQL for the
manifest dialect where the step is mechanically reversible (add⇄drop, rename⇄rename, create⇄drop
table) and a loud `-- <step> is irreversible (data loss); write your own or delete this file` for the
rest — so the file exists and invites completion (without an invitation, down migrations are never
written).

(f) **Document the raw/snapshot boundary.** raw touching an object the snapshot *models* (a
table/column/index) makes the snapshot blind with no shadow-DB to catch it; raw on *unmodeled*
objects (views, triggers, extensions) is safe blindness. migrations.md/raw.md document the boundary;
a `W0109` lint flags a raw migration step naming a modeled table (lower-priority within this slice).

Spec seam: migrations.md (E5 + raw structural effects), raw.md, editors/vscode. Runtime
`MigrateError::UpMigDrift`; lint `W0109`.

**Shipped.** All six landed. (a) `render_up`'s header (`migrate/up_mig.rs`) states the real
contract — structural steps derive from `schema.snap`, editing one has no effect, the editable
surface is `raw(<dialect>)` lines (which run after all structural steps, any file position) + a
hand-authored `down.mig`; every header line is a `#` comment, so the tamper hash is unchanged. (b)
One shared drift check, `migrate::up_mig_matches_snapshot` (`content_hash(strip_raw_steps(up)) ==
content_hash(render_up(steps))`), is called by runtime `load_migrations` (→ `MigrateError::UpMigDrift`,
refusing apply/status), CLI `render` (→ a CLI error), and `verify` (which already flagged it) — so a
structural hand-edit is refused at the moment of harm, not silently ignored; it canonicalizes like the
content hash (cosmetic edits tolerated) and strips `raw` lines first (a `raw`-line edit is Tamper, not
drift, keeping both guards meaningful). (c) `.mig` + `.snap` tmLanguage grammars
(`editors/vscode/syntaxes/{mig,snap}.tmLanguage.json`, embedded `source.sql` in raw blocks, a loud
`# DESTRUCTIVE` scope) + two `languages`/`grammars` contributions in `package.json`. (d) `parse_raw_steps`
/`strip_raw_steps`/`has_raw_step` rewritten onto one block-aware `scan_raw`: a `raw(<dialect>)` opener
takes either a single-line SQL (closing backtick on the same line) or a multi-line block (opening
backtick last on the line, closing backtick alone on its own line); the whole `up.mig` stays hashed, no
sidecars. (e) `gen` writes `down.mig` via new per-dialect `migrate::render_down` — real reverse SQL for
mechanically reversible steps (add⇄drop column/index, create⇄drop table, rename⇄rename), a loud
`-- <step> is irreversible (data loss); write your own or delete this file` for the rest; `load_migrations`
treats an all-comment (zero-statement) `down.mig` as absent, so an untouched placeholder stays
roll-forward-only (a `--down` is a loud `NoDown`, not a silent no-op). (f) migrations.md + raw.md document
the safe (unmodeled: views/triggers/extensions) vs dangerous (modeled: table/column/index) raw boundary;
`W0109` (`migrate::raw_modeled_tables`, whole-identifier scan) surfaces in `based migrate verify` when a
raw step's SQL names a modeled table. Tests: codegen units (honest header, multi-line raw round-trip,
drift helper, down-prefill, `raw_modeled_tables` word-boundary), runtime (UpMigDrift refused at load,
multi-line raw applies live on SQLite, the two Tamper tests reworked to a `raw`-line append), CLI
(down.mig prefill + irreversible placeholder, W0109 in verify). Green via `make check`.

## D107 — named `tx` step bindings replace `^` (NF13)

**Decision.** A `tx` step back-references a prior step only via `^.field`, which today (sema `prev`
reassignment in `check.rs` + a single-slot codegen `BackCtx`) reaches **only the immediately
preceding `create`** — so a 3-step tx referencing step 1 is unwritable — and `^.id` is unintuitive +
ungreppable. Replace it with **named step bindings**: bind a step’s produced row with `as <name>` and
reference it as `$name.field`, unifying `$` as “a value bound in this callable” (params + step
bindings):

```
tx {
  create User { email = $email } as user;
  create Address { user = $user.id, city = $city };
  create Log { actor = $user.id };   // reaches step 1
}
```

`as <name>` is a keyword (the bare trailing form `create … user;` is two adjacent bare tokens —
banned by principle 3). Reference is **field-access only** (`$user.id`) and **single-assignment** (no
rebinding), so no Turing-creep (principle 5). A binding reaches **any prior step** in the tx (fixes
the step-1 gap). A binding whose name shadows a param, or a duplicate binding, is **`E0280`**; a
`$name` naming no prior binding (or a later/forward step) is **`E0281`**; `$name.field` where `field`
isn’t on the bound step’s model reuses `unknown_field`. **`^` is removed entirely** — pre-release, no
back-compat shim (owner: “bin it off mercilessly”): the `Tok::Caret` / `Value::Back` / `BackRef` /
`BackCtx` machinery and `E0170` all go; a `^` in source is a parse error whose message points to
`create … as <name>;` + `$name.field`. Spec seam: mutations.md Atomic groups + grammar.ebnf; the
helpdesk `open_ticket` (uses `^`) + conformance goldens migrate to `as`. Codes `E0280`/`E0281`;
`E0170` retired.

**Shipped.** `create … as name` binds after any `on conflict` tail (`create_stmt` in grammar.ebnf);
the reference is an ordinary `ParamRef` (`$user.id`) — no new AST `Value` variant, so `$` genuinely
unifies params + `$ctx` + step bindings at parse time, and sema/codegen disambiguate by name (a
`$name` that is neither `$ctx` nor a declared param is a step binding). Sema threads a `Bindings`
env (`resolved`: name→model reachable *now*; `all`: every binding in the tx, so a forward reference
reads distinctly from a typo) — E0280 shadow/dup on the binding decl, E0281 unbound/forward at the
reference, `E0111` for an unknown field on the bound model, `E0153` for a family clash typed through
the bound field. Codegen keeps D16's lowering exactly — sibling creates still get distinct `:id_<step>`
binds and `$name.id` resolves to the bound step's `:id_<step>`, a non-`id` field reuses that create's
assigned value — but keyed by a `BackCtx` **map** reaching any prior step (was a single slot). The
runtime is unchanged (it already binds every create's `gen_id`). `^` is gone from the lexer, so a bare
`^` lexes as an unrecognized byte and the parser renders it as an `E0001` pointing at `as`. Editor
surface: a binding decl (`as name`) and every `$name.field` head are one callable-local symbol —
go-to-def, find-refs, rename, and a hover naming the bound model. Migrated: helpdesk `open_ticket`;
tests across parser/sema/codegen/fmt/runtime/LSP + a new sema conformance golden `tx_bindings`. Live
via `make check` (the `open_ticket` tx ran green against Postgres/MariaDB).

## D108 — opt-in FK referential actions: `@fk` / `@no_fk` + a `foreign_keys` convention (T6)

**Decision.** A relation's `<field>_id` FK **column** is always stored, but the DB `FOREIGN
KEY` **constraint** is opt-in and every divergence from the project convention is visible in
source (FK constraints are often banned at scale — principle 2). Owner-approved syntax:

- **toml `[schema] foreign_keys = "all" | "none"`** (default `"none"`, backward-compatible).
  `"none"`: a relation gets a constraint only if it writes `@fk`. `"all"`: every forward
  relation gets a bare FK unless it (or its model) writes `@no_fk`. Per-relation/per-model
  decorators always win over the toml default.
- **`@fk` — opt a forward (to-one) relation IN**, with optional standard-SQL actions:
  `@fk(on_delete: cascade)`, `@fk(on_delete: restrict, on_update: cascade)`, bare `@fk`
  (DB-default action, no clause). Actions: `cascade`, `restrict`, `set_null`, `no_action`;
  `on_delete:`/`on_update:` are independent optional kwargs.
- **`@no_fk` — opt OUT**, on one forward edge (`actor: User @no_fk`) or a whole model
  (`Order @no_fk { … }` — every forward relation).

**The divergence-reason rule (the load-bearing part).** The toml value is the project's
convention. A **reason string is required exactly when a decorator flips FK presence AGAINST
that convention** — spelled/handled identically to `@no_id("reason")` (a leading positional
string). Under `"none"`, `@fk` *adds* an FK → reason required (`E0295`), and `@no_fk` is
redundant (`W0110`). Under `"all"`, `@no_fk` *removes* an FK → reason required (`E0295`), a
bare `@fk` is redundant (`W0110`), and `@fk(on_delete: …)` refining a present FK is
concordant (no reason). Actions never trigger a reason on their own — only flipping presence
does. Because it depends on the manifest, this runs in a **manifest-dependent pass mirroring
D104's `check_target`**: `check_foreign_keys(&schema, foreign_keys)`, run by the CLI with the
manifest value and the LSP with the resolved project value (the dialect-free `check` can't
decide divergence alone).

**Other checks (convention-free, in `check`).** `@fk`/per-edge `@no_fk` is valid only on a
forward to-one relation: on an inverse/`[]`/scalar → `E0290`; on a custom-join (`on:`)
relation (no conventional FK column) → `E0291`; `@fk` + `@no_fk` on one edge → `E0292`;
`on_delete: set_null` on a required (non-nullable) relation → `E0293`; an unknown action →
`E0294`. A forward relation to a `@no_id` keyless target is already `E0265` (no PK to
reference) — unchanged.

**Codegen.** DDL emits `CONSTRAINT fk_<table>_<col> FOREIGN KEY (<col>) REFERENCES
<ref>(<id>) [ON DELETE <a>] [ON UPDATE <a>]` inline on all three dialects (SQLite honors an
inline table FK), resolved per relation via `RModel::resolved_fk(mem, foreign_keys)`. FK
presence threads through `sql::ddl_with` / `Snapshot::from_schema_with` (the old
`ddl`/`from_schema` keep the safe `none` default, so an explicit `@fk` still emits under it
and callers that don't thread the convention are unaffected). **SQLite enforcement:** the
`foreign_keys` pragma is set ON explicitly at connection setup (sqlx defaults it on; made
explicit + greppable), so `on_delete: cascade` actually cascades — proven live.

**Snapshot + migrations.** A resolved FK is a `fk <col> -> <ref_table>.<ref_col>
[on_delete=…] [on_update=…]` line in `schema.snap` (a new `ForeignKeySnap` alongside
`IndexSnap`), so adding/removing/changing an FK diffs into an `add foreign_key` /
`drop foreign_key` step (a changed action is drop + re-add). Postgres/MariaDB render these as
`ALTER TABLE … ADD CONSTRAINT`/`DROP CONSTRAINT`/`DROP FOREIGN KEY`; **SQLite has no in-place
FK ALTER**, so an add/drop there is an honest loud marker pointing at a hand-authored
`raw(sqlite)` rebuild — never a silent skip (the full-rebuild engine is out of scope this
iteration; from-scratch `create table` carries FKs inline on SQLite, so init works).

**Codes.** `E0290` (bad target), `E0291` (custom-join), `E0292` (fk+no_fk conflict), `E0293`
(set_null on required), `E0294` (unknown action), `E0295` (missing divergence reason), `W0110`
(redundant decorator). Spec seam: relations.md (the real `@fk`/`@no_fk` spec, replacing the
"no FK unless asked" hand-wave), models.md (decorators), migrations.md (FK diff/step).

**Shipped (2026-07-23).** Implemented end to end as decided. Notes: `check` runs the
structural pass (`validate_fk`); `check_foreign_keys` is the manifest-dependent divergence
pass. `MemberKind::Forward` gained an `FkDecl` (presence intent + resolved actions + reason
spans); `RModel` gained model-level `no_fk`. Tests: parser +/− (`@fk`/`@no_fk` forms), sema
+/− per code **in both toml directions incl. both redundancy lints** (`tests/fk.rs`, 18
cases), DDL golden all three dialects, snapshot round-trip + add/drop/change diff (+ per-dialect
render, incl. the SQLite honest-marker), fmt round-trip, a sema conformance golden
(`fk_referential`), and a **live SQLite cascade proof** (bad-parent insert rejected → pragma
on; parent delete cascades the child away). `make check` green end to end (fast gate + all
three live suites + all examples + the axum-helpdesk smoke).

## D109 — many-to-many far-side flattening projection (`courses = enrollments.course { … }`) (T5)

**The gap.** A shape could reach the far side of an explicit-junction m2m (D102) only
*through* the junction — `enrollments { course { title } }` returns a `Vec` of junction
wrappers, each holding one course. Consumers want the far side directly as a flat list. The
**far-side flattening projection** hides the junction and returns the far side as a
`Vec<far-shape>`. This is the last open T5 slice; **implicit-junction sugar
(`Course[] <-> students`) stays rejected** (D102/D103 — an engine-generated join table is
PR-invisible DDL, wants explicit-in-source resolution).

**Syntax (owner-approved).** A shape field spelled with `=` (a derived field, like an
aggregate — *not* a stored `:` field) naming a relation **path** through a to-many inverse
edge then forward edge(s) to the far side, with a projection body:
```
StudentCourses from Student {
  name
  courses = enrollments.course { title }   # -> courses: [ { title }, … ], junction hidden
}
```
`enrollments.course` = hop into the to-many junction (`enrollments`, an inverse edge on the
`from` model), then out a forward edge (`course`) to the far model; the body is the far
model's projection; the field is `Vec<Course>`. Generalizes to N hops (inverse edge first,
then forward edges; the **last** segment's model is the element type), but the 2-hop
junction-skip is the primary/tested case. Parses to `ShapeField::Flatten { out, path, body }`
— a brace body after `= path` distinguishes it from a plain `= path` reach.

**Semantics.**
- **Distinct far-side rows** — the list is the *set* of related far rows, each once (distinct
  on the far PK), so a junction's link cardinality never leaks (a duplicate link, or a filter,
  can't multiply a far row).
- **Order unspecified** unless the far model declares `@sort` (portable JSON aggregation has no
  cross-dialect ordered form) — the same rule as a to-many nest (D57/D87).
- **Scope + soft-delete ride the subquery** — the far model's `@scope`/`@soft_delete` **and**
  the junction's own, injected into the right level (junction's into the inner `IN`, far's into
  the outer `WHERE`). Nesting into a scoped far side counts as *touching* it (E0185 territory,
  D81) — the scope walks (`walk_shape_join_in`/index demand) recurse the flatten path + body.
- **Composes** — the far body may itself nest / flatten further (recurses).
- **Keyless far model** (`@no_id`) → `E0302` (no PK to dedup the distinct set on).

**Lowering (a two-level correlated subquery — the portable DISTINCT route).**
```sql
(SELECT <json-agg>(<json-object of body>) FROM <far> AS s2_far
 WHERE s2_far.id IN (
   SELECT s1_junction.<far_fk> FROM <junction> AS s1_junction [<intermediate joins>]
   WHERE s1_junction.<near_fk> = <outer>.id AND <junction soft-delete/@scope>)
 AND <far soft-delete/@scope>)
```
Iterating **far rows** (`FROM far`, `far.id IN (…)`) gives DISTINCT-on-PK for free on every
dialect — a plain `IN`, no `json_agg(DISTINCT …)` / `SELECT DISTINCT` gymnastics (Postgres
`json` has no equality operator, so a `DISTINCT` on the built object wouldn't compile; and
DISTINCT-on-the-object is the wrong key anyway — two distinct far rows with equal projected
columns must both appear). Reuses D57's per-dialect `json_array_agg` (SQLite
`json_group_array`, MariaDB `JSON_ARRAYAGG`, Postgres `json_agg`, all coalesced to `[]`), the
`s<n>_<table>` child aliasing (so a self-referential m2m never collides), and the `field[]`
[`ARRAY_MARK`] output alias — so the **runtime is unchanged** beyond what D57 provides (it
already parses a `field[]` column into sub-objects). Element order = the far model's `@sort`
inside the aggregate.

**Sema (three stable codes, E030x).** `E0300` the path's first segment is not a to-many
inverse edge (nothing to flatten through). `E0301` a later segment doesn't resolve as a
forward edge to the next model, or the path is single-segment (no far side). `E0302` the far
model is `@no_id` (keyless). The body is validated like a nest body (`check_shape_body`
against the far model). A flatten on an aggregate shape is `E0245`; a flatten in a raw-bodied
query's shape is `E0213`.

**Landed across:** AST (`ShapeField::Flatten`); parser (`= path { body }`); sema
(`check_flatten_path` + E0300–E0302, the scope-touch walk in `scope.rs`, index demand in
`indexes.rs`); SQL codegen (`Select::json_flatten_subquery` + `spawn_child`, reached from
`project_body` and `json_object_expr`); client (`Vec<Sub>` far element struct, the junction
hidden) + OpenAPI (array of the far object schema); fmt (`out = path { body }`, inline +
block, aligned); LSP (path segments navigable, body reaches the far model). **Verified:**
parser +/−, sema +/− per code + the E0185 touch case, codegen SQL all three dialects (the
two-level IN subquery, junction+far soft-delete/scope split across levels, self-ref
aliasing), client/OpenAPI, fmt round-trip, a sema conformance golden (`m2m_flatten`), and
**live SQLite** — a distinct `Vec<Course>` with the junction hidden, a course shared across
two students, a duplicate link deduped to one far row, a soft-deleted junction link excluded,
and a soft-deleted far course excluded. `make check` green end to end. Spec:
`spec/syntax/relations.md` (Many-to-many) + `spec/syntax/shapes.md`.
