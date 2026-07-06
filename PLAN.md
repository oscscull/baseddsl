# PLAN.md — build-out roadmap

Working notes for whoever picks this up next. Records what's **done**, what's
**deferred** (with enough context to resume without re-deriving), and the
**remaining milestones**. Spec is truth for *what* the language is; this is truth
for *where the implementation stands*.

## Autonomous build loop (how this is being built out)

This roadmap is executed by a self-driving loop. Protocol, for whoever (human or agent)
resumes it:

- **One item per iteration, in fresh context.** Each iteration spawns ONE fresh
  general-purpose subagent that reads CLAUDE.md + `spec/principles.md` + this file +
  `spec/decisions.md`, picks the **highest-leverage unstarted item**, implements it fully,
  and commits it. A fresh subagent per item is what keeps context clean between iterations
  (the whole point); the coordinator retains only one-line summaries, never the work.
- **Gate before commit.** `cargo test --workspace --all-features`, `cargo fmt --check`, and
  `cargo clippy --workspace --all-features` must all be clean. Never commit red.
- **Commit style.** On the current working branch (no push, no PR): first line
  `m6: <desc> (D<n>)`, short body, ending with the trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Update this file
  and `decisions.md` in the *same* commit as the code.
- **Sequential only.** Each iteration commits before the next starts (so the next reads
  updated state). The coordinator NEVER touches the repo/git while a subagent is running —
  they share one working tree + index.
- **Pause** after 3 items in a batch, or when a subagent hits a genuine blocker (it stops
  WITHOUT committing and reports), or when the unstarted items are exhausted.

Batch progress: current batch — D29 (Postgres dialect: `ddl`/`dml`/`mutations` codegen +
the dialect-aware `?`→`$n` scanner; the concrete driver deferred to the live-DB slice) done — 1/3.
Prior batch: D26 (container probes + graceful shutdown) + D27 (SQLite backend + real integration) +
D28 (SQLite DDL codegen) — 3/3.

## Pipeline (data flow)

```
*.bsl ──manifest::discover──▶ files
      ──parser::parse_file──▶ [Decl]           (per file; recovers at decl boundary)
      ──sema::check─────────▶ CheckedSchema + [Diagnostic]
      ──codegen::sql::ddl───▶ SQL DDL          (M2 ✅; dialect-aware: MariaDB + SQLite + Postgres, D28/D29)
      ──codegen::sql::dml───▶ query SELECTs    (M3 read side ✅)
      ──codegen::sql::mutations─▶ INSERT/UPDATE/DELETE  (M3 write side ✅)
      ──codegen::client─────▶ typed Rust client (M4 ✅)
      ──codegen::openapi────▶ OpenAPI 3.1 doc → polyglot clients (D24 ✅)
      ──facts───────────────▶ engine-derived facts    (M5 ✅)
                              └─ based-lsp ──▶ editor inlay hints + hover + diagnostics
      ──runtime::plan/run───▶ bound positional statement + shaped JSON  (M6 read+write ✅)
      ──runtime::serve──────▶ WireResponse (dispatch core; PlanError→4xx, DbError→503)  (M6 ✅)
      ──runtime::http───────▶ `based serve`: tiny_http listener over dispatch  (M6 ✅ D21)
                              └─ /healthz + /readyz probes + graceful drain (M6 ✅ D26)
      ──runtime::embed──────▶ in-process Engine (socket-free dispatch; typed client seam)  (M6 ✅ Tier 1)
      ──runtime::sqlite─────▶ infra-free SqliteDb/Backend → real in-memory integration tests  (M6 ✅ D27)
```

`based check` wires discover → parse → sema → render. `based gen sql [--out]` runs the
same front end (`load_checked` in based-cli), then lowers the `CheckedSchema` to DDL,
then appends the query SELECT templates (`sql::dml`) and the mutation write templates
(`sql::mutations`), both reading the AST alongside the IR. `based gen client [--out]`
runs the same front end, then lowers to a typed Rust client module (`client`). `based gen
openapi [--out]` runs the same front end, then emits one OpenAPI 3.1 document over the
same wire (`openapi`, D24) — feed it to `openapi-generator` for a client in any language.
All bail unless every file parses *and* checks clean (codegen assumes a clean schema).

## Crate status

| crate | state | notes |
|-------|-------|-------|
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). Missing: schema-version. (`$ctx` is inferred in sema, not declared here — D4.) |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. |
| **based-sema** | ✅ **this milestone** | resolution + checks + lints + `CheckedSchema` IR. Details below. |
| based-cli | ✅ works | `based check` + `based gen sql` (DDL + query SELECTs + mutations) + `based gen client` (typed Rust) + `based gen openapi` (OpenAPI 3.1, D24) + `based facts [--json]` (derived facts, M5) + `based serve` (HTTP listener, D21). |
| **based-codegen** | ✅ **M2 (DDL) + M3 (read+write) + M4 (client) + OpenAPI (D24)** | `sql::ddl` → `CREATE TABLE`, **dialect-aware (MariaDB + SQLite + Postgres, D28/D29)** — per-dialect type map + inline (MariaDB) vs. separate `CREATE INDEX` (SQLite/Postgres); `sql::dml` → query SELECTs (`lower_queries` seam); `sql::mutations` → INSERT/UPDATE/DELETE (soft-delete rewrite + scope injection; `lower_mutations` seam feeds both the text emitter and the runtime); `client` → typed Rust client (inputs/outputs/routes); `openapi` → one OpenAPI 3.1 doc over the same wire (polyglot clients via `openapi-generator`, D23/D24). The `Dialect` enum now has **three** variants; the DML/mutation SQL branches through one `Dialect::quote`/`qcol` quoting seam + a few operator/literal spellings (`= TRUE`, `MEMBER OF` vs `@>`) + Postgres's `FROM`/`USING` multi-table write restructure (D29). |
| **based-facts** | ✅ **M5** | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>`: the "show, don't write" facts — inferred inverse pairings, join-key indexes, per-callable `$ctx` requirement bags, and each query's resolved shape (verb/target/cardinality/pagination) — span-anchored. Golden/unit-tested; consumed by the CLI + LSP. |
| **based-lsp** | ✅ **M5** | tower-lsp server. Recompiles on edit (discover→parse→check, unsaved buffers overlaid on disk), publishes diagnostics + inlay hints + hover from `based-facts`. |
| **based-runtime** | 🚧 **M6 (read + write + dispatch + driver core + HTTP listener)** | in-process engine (D18). `Compiled::load` reuses the front end + codegen's query *and* mutation lowering; `plan_query`/`plan_mutation` validate args/`$ctx`, bind `:name`→the dialect's positional form (`?` on MySQL/MariaDB/SQLite, `$n` on Postgres — D29, via the `Compiled.dialect` carried from the manifest), pick the response envelope (reads) / generate engine ids + thread `^` back-refs (writes); `run_query` shapes rows, `run_mutation` executes writes under one `begin`/`commit` and re-selects a create's declared shape as the response (D12). `serve::dispatch` is the wire core (`POST /q\|m/<name>` → `WireResponse`; PlanError→4xx, DbError→503), mock-tested. `Db` is now **fallible** (rollback-on-failure). Concrete `MariaDb` driver + bounded-pool `ShardRouter` behind feature `mariadb` (D20). **HTTP listener `http` (feature `serve`, D21)**: sync bounded worker pool over `tiny_http`, `ContextSource` (`$ctx` from headers), production `UuidGen`, driver-neutral via the `Backend` seam; `based serve` CLI. **In-process door `embed` (Tier 1, D22)**: `Engine` (`Compiled` + one `Db` + `IdGen`) runs a callable through `serve::dispatch` with no socket, backing the *same* typed generated client via a tiny `impl Transport`; worked end-to-end example in `tests/embed.rs`. **Write-retry idempotency `idempotency` (D25)**: a keyed mutation runs its write body at most once per `(callable, key)` — a retry replays the recorded response instead of double-inserting; `IdempotencyStore` seam (`MemStore` in-process / `NoStore` no-op), key on the `Idempotency-Key` header (never body / `$ctx`), in-flight duplicate → retryable `409`; threaded through `dispatch` / HTTP edge / `Engine::call_with_key`. **Container story `serve` (D26):** operational probes `GET /healthz` (liveness, DB-free) + `GET /readyz` (readiness via the new defaulted `Backend::ping`; `ShardRouter` probes every shard with `SELECT 1`) answered ahead of routing, plus **graceful shutdown** — `serve_with_handle` returns a `Handle` whose `shutdown()` flips a *draining* flag (readiness fails first → LB drains) and lets in-flight requests finish before workers exit; the SIGTERM/SIGINT→drain wiring lives in the CLI (`ctrlc`), keeping the library signal-free. **SQLite backend `sqlite` (D27):** `SqliteDb`/`SqliteBackend` over bundled `rusqlite` — the infra-free concrete `Db`/`Backend` (shared in-memory connection, no shards, `ping`=`SELECT 1`), backing the first **real** end-to-end integration tests (`tests/sqlite_integration.rs`): the actual commerce schema's *verbatim* lowered SQL run through `dispatch` against a live engine (get/list/`$ctx`/write+re-select/`ping`), no `MockDb`. **Postgres codegen + `$n` scanner done (D29) — the concrete `postgres` driver is deferred to the live-DB slice; Live-DB (MariaDB/Postgres) integration + a container image/Dockerfile not started (architecture ready, D21/D22/D26/D27/D29); durable multi-instance idempotency store deferred to the live-DB slice (D25).** |

## based-sema — what it does now

Entry: `check(&[Decl]) -> (CheckedSchema, Vec<Diagnostic>)`.

Modules: `ir` (resolved types + codes + `Sink` + `snake_case`), `model` (AST model
→ `RModel`, two-phase), `resolve` (path resolution + the shared predicate/value
checker + `Cx` context), `check` (shapes/queries/mutations/filters + the four query
inferences), `ctx` (`$ctx` per-callable inference + coherence, D4/D5), `indexes`
(inferred-index model + the index lints, D15), `lib` (orchestration).

Pass order (see `lib.rs`): collect+dedup → skeletons → validate (mut) → resolve
exprs (read-only) → check shapes/queries/mutations/filters. Split into mut/read
passes because scope/sort path resolution traverses *other* models while validate
holds `&mut`.

**Implemented checks**

- Operand type-checking (sema #1, done): op/operand applicability + operand family
  compatibility in `Cmp` (`E0150`/`E0151`); param annotation vs. mapped column
  (`E0152`, D1). See resume-points list below for the exact shape.
- Name resolution: relation targets, inverse pairings (explicit `(M.field)` and
  inferred from the unique forward edge), shape `from`, return types, statement
  models, mutation write models, dotted paths (forward + backward traversal),
  index columns, `$param` refs (`$ctx.<field>` structural check; its type is
  inferred per callable from use + checked for coherence, D4/D5), filter calls + arity
  *and* their bodies re-resolved against the call-site model (D14, cycle-guarded),
  functions (closed set `KNOWN_FUNCS`), `^.field` tx back-references (D16: resolved
  against the immediately preceding `create`; `E0170` outside a tx / no prior create),
  custom `on:` join predicates (D17: two-table scope — FK-holding model + target —
  table-qualified physical columns; `E0125` bad table, `E0126` malformed).
- `create` required-field enforcement: every non-optional, non-defaulted column /
  forward FK must be assigned (`E0146`); engine-managed fields (`id`, `@created`/
  `@updated`, `@soft_delete`) and custom-join forwards are exempt.
- `create`/`update` assign type agreement (`E0153`): the assigned value's family must
  match the target column — the write-side twin of the `=` operand typing. Literals and
  columns are family-checked; a `^` back-reference is typed by the field it reads on the
  preceding create; params (typed at declaration / `$ctx` inferred) and functions are
  skipped, exactly as on the read side.
- Implicit `id: Id` (D2); a model that declares its own `id` keeps it.
- Decorators: `@soft_delete` (covered-subset type check → `SoftMode`), `@created`/
  `@updated` (timestamp role), `@scope` (predicate, `$ctx`-only), `@sort`
  (paths), `@table` (name override). Unknown `@foo` → `W0101`.
- Table naming (D3): `snake_case`, no pluralization, `@table("…")` override.
  Relation FK column = `<field>_id` or `(column "…")`.
- Query inferences (queries.md): target model (from return shape's `from`), verb
  (`get`/`list` explicit in block, else from cardinality), param→same-name column
  mapping (bare/inline), per-param bindings (`-> edge`, `op col`).
- `get` must be keyed on a unique field → `E0144`.
- Duplicates: model / shape (except `full`) / callable (query+mutation share the
  wire namespace) / filter / field.
- Lints: `W0100` nondeterministic `list` (no sort at any tier), `W0102` raw SQL on
  a `@soft_delete` model (tombstone gap).
- Index inference + lints (indexing.md, D15, `indexes.rs`): per-query *and* per
  mutation-`where` access patterns (eq/range/sort off the conjunctive spine, params
  + `@scope` + call-site filter bodies included) vs. available indexes → `W0103`
  missing-index (satisfied by `@index` or the `unindexed(max_rows: N)` /
  `unindexed(unsafe)` query clause; a bulk `update`/`delete` scans the same way but
  has no such clause, so it simply shows; `W0105` when a query annotation goes
  stale); pooled usage (queries + mutation `where`s) → `W0104` useless-index.
  Traversed inverse edges seed `RModel.inferred_indexes` (join-key baseline, DDL
  emits them `inf_`-prefixed, soft-delete predicate-leading).

**Diagnostic codes** live in `ir::code` (E01xx errors, W01xx lints). Parser owns
E0001/E0002, manifest E001x. Codes are stable — grep `ir.rs` for the registry.

**`CheckedSchema`** (the codegen seed): `models: Vec<RModel>` (fully resolved:
table name, members with kind Scalar/Forward/Inverse, soft_delete mode, sort,
scope, created/updated, indexes, unique_cols), plus resolved summaries
`shapes/queries/mutations/filters` and a `model_index` map. Codegen reads this
alongside the AST (`RQuery` carries inferred verb/target/many/paginated that are
*not* in the AST).

Tests: `crates/based-sema/tests/check.rs` (81 cases, positive + negative, keyed on
diagnostic codes), plus `tests/conformance.rs` — a golden harness over
`tests/conformance-sema/<case>/` that pins the resolved-schema summary + diagnostics
(resume #8; re-bless with `BLESS=1`). Commerce example (`spec/examples/commerce`)
checks clean (including a `$ctx.org` query whose context is inferred with zero
config, D4/D5).

## based-sema — deferred (resume points)

Ordered by value. Each is a real gap with a known approach.

1. ~~**Operand type-checking.**~~ ✅ **done.** `resolve::check_cmp_types` now consumes
   the `Terminal` payload: op/operand applicability (`~` needs text → `E0150`;
   `< > <= >=` need an orderable column, not bool/json/relation → `E0150`) and
   family compatibility for `=`/`!=`/ordering against a literal *or* another column
   (`age = "x"`, `qty = name` → `E0151`). Type families are coarse on purpose
   (Timestamp/Date/Uuid/Id ride with text; Json matches anything; a relation key
   accepts a uuid string or int, D1). Param explicit-type vs. mapped-column
   agreement is `resolve::check_param_type` (D1: a relation param may be typed the
   target model *or* a key `Id`/`Uuid`; scalar params match by family → `E0152`),
   wired through `check::check_param`'s new `mapped_member`. `in`/`has` operand
   typing is deliberately skipped (collection/json element type differs from the
   column — needs the `many`/element model, not yet on `Terminal`). Tests: 11 new
   cases in `check.rs` (40 total).
2. ~~**Named-filter body resolution.**~~ ✅ **done** (D14). A `filter` still declares
   no model, but its body is now re-resolved against each *call-site* model in
   `resolve::resolve_filter_body` (reached from the `FilterCall` / bare-atom arms of
   `check_predicate_in`), with the filter's own params as the legal `$`-set and an
   `in_filters` stack guarding self-reference. Column errors, traversal errors, and
   operand typing all fire against the real caller model. Decided the `$c` question:
   filter params are `$`-referenced (grammar already required it; spec example
   corrected). Tests: 5 new cases in `check.rs` (45 total). **Codegen lowering now
   done too** (see M3 read): a `FilterCall`/bare-filter atom is inlined — args
   substituted through the body, lowered against the call-site model, joins and all;
   self-reference guarded with a visible `/* filter … recursion */` marker. *Still
   deferred*: arg-vs-usage type agreement (filter params carry no declared column).
3. ~~**Index lints (indexing.md).**~~ ✅ **done** (D15, `indexes.rs`). The inferred
   baseline is *traversed join keys only* (inverse-edge FK columns — the one class
   that is unambiguously right to auto-create; DDL emits them `inf_`-prefixed,
   soft-delete column prepended since MariaDB has no partial indexes). Filter-path
   indexes are shown via `W0103` missing-index instead of auto-created (write tax
   is a human call, principle 8): per-query eq/range/sort pattern vs. first column
   of any available index; `or`/raw patterns are opaque → silent (precision over
   recall). Satisfied by `@index` or the new `unindexed(max_rows: N)` /
   `unindexed(unsafe[, "reason"])` *query clause* (grammar + AST + parser);
   `W0105` flags a stale annotation. `W0104` useless-index fires on a declared
   non-unique index whose lead nothing filters/sorts/joins on (broad usage pool,
   under-fires by design; unique indexes exempt; single-col duplicate of a
   `(unique)` constraint always flagged). Mutation `update`/`delete`/`restore`
   `where`s now feed the same pool: an unindexed bulk write draws `W0103` (no
   `unindexed(…)` clause exists on a write, so it just shows), and a column a
   mutation filters on counts as used for `W0104`; tests in `check.rs`. *Still
   deferred*: composite-prefix matching; prod-stats floors + `max_rows` re-checking;
   the `unsafe` audit listing; LSP surface (M5).
4. ~~**`$ctx` typing (D4/D5).**~~ ✅ **done — by inference, not declaration**
   (`based-sema::ctx`). `$ctx` is per-request: there is no global context type. Each
   callable *requires* exactly the `$ctx.<field>`s it reads (its `where`, its target
   model's `@scope`, expanded filter bodies, `create`/`update` assigns), and each
   field's type is **inferred from the column the use compares against** — the same
   inference untyped query params already use. `ctx::collect_query`/`collect_mutation`
   attach a deduped `Vec<CtxReq>` to each `RQuery`/`RMutation` (the client will send
   exactly these). The one global fact is **coherence** (`ctx::check_coherence`,
   closed-world): a field name must mean one type everywhere the caller's shared
   context bag is read → `E0161` on a clash (across *or* within a callable).
   `resolve::check_param_ref` enforces the structural rule (`$ctx.<field>`, one
   segment → `E0160`). No manifest `[ctx]`, no config: commerce's `my_org_orders`
   (`where (org = $ctx.org)`) checks clean and lowers to `WHERE order.org_id =
   :ctx_org` with zero declaration. Tests: 9 new in `check.rs` (67 total).
   *Deferred residue*: a `$ctx` field with no column to infer from — used only in a
   `guard` (Handle 3, which takes no args yet) or a raw block — is typed by a local
   annotation *at the use site* when `guard` grows args (decided direction, D4); it
   contributes nothing to inference today. Also deferred: `$ctx` passed *as a filter
   arg* (arg/usage typing, D14); emitting the per-callable `Ctx` type in the client.
5. ~~**Relation `on:` custom joins.**~~ ✅ **done** (D17). A forward relation's
   `(on: order.user_ref = user.legacy_id)` predicate is now resolved in a *two-table*
   scope — the FK-holding model plus its target — in `model::resolve_exprs` (read
   pass, where other models are reachable). `resolve::check_relation_on` walks the
   join predicate; each column path must be `<table>.<column>` naming one of the two
   tables in scope (`E0125` otherwise) and a real *physical* column on it (matched via
   the new `RModel::column`, `E0111` otherwise). A join is static structure, so
   `$`-params / filter calls / `^` back-refs / bad arity are `E0126`; `on:` on a
   non-to-one field is also `E0126`. Tests: 6 new in `check.rs` (81 total). *Still
   deferred*: self-ref join aliasing at codegen (resolution treats both sides as the
   one model); lowering the custom `on:` predicate into the emitted JOIN (codegen twin
   — today codegen still joins on the convention `fk_col`).
6. ~~**`^` tx back-references (mutations.md).**~~ ✅ **done** (D16). Full vertical
   slice: lexer `^` token, AST `Value::Back(BackRef)`, parser `back_ref` in value
   position, sema resolves `^.field` against the *immediately preceding `create`* in
   the enclosing `tx` (`check::check_back`; `E0170` when there is no prior create or
   `^` is used outside a tx / in a predicate, `E0111` for an unknown field), and
   codegen (`sql::mutations`): sibling creates in a tx get distinct id binds
   (`:id_<step>`) so they don't collide, and `^.id` binds the prior create's id
   (`^.<other>` reuses that create's assigned param/literal). Tests: 4 sema, 1 parser,
   2 codegen. *Still deferred*: `^.field` for a field the prior create didn't set
   (needs a re-select / RETURNING, a runtime concern) emits a `NULL /* … */` marker;
   multi-level `^^`. (Back-ref *type* agreement with the assigned column is now done —
   see resume #7, `E0153`.)
7. ~~**create/required-field enforcement.**~~ ✅ **done.** `check::check_create_required`
   now verifies a `create` assigns every *required* column — a non-optional,
   non-defaulted scalar or forward FK — reporting all missing fields in one
   `E0146`. Engine-managed fields (`id`, `@created`/`@updated`, the `@soft_delete`
   field) and custom-join forwards (no FK column) are exempt; inverse edges own no
   column so they never count. Tests: 3 new in `check.rs`; commerce `place_order`
   grew a `total: int` param (its `create` had silently omitted the required
   `total`). ~~*Still deferred*: back-ref/assign *type* agreement with the target
   column (D16 residue).~~ ✅ **done** — `resolve::check_assign_type` (`E0153`) now
   family-checks every `create`/`update` assign, `^` back-references included (typed by
   the field they read on the preceding create). Tests: 4 new in `check.rs` (85 total).
8. ~~**Sema conformance goldens.**~~ ✅ **done.** `crates/based-sema/tests/conformance.rs`
   mirrors the parser harness against a sibling case dir `tests/conformance-sema/<case>/`
   (`input.bsl` + `expected`); re-bless with `BLESS=1 cargo test -p based-sema --test
   conformance`. The summary is the resolution facts *not* in the AST — table names,
   relation kinds (`-> T fk=…` / `<- T via …`), soft-delete mode, `@scope`/`@sort`,
   declared + `inferred(...)` indexes, inferred verb/target/many/shape/paginated, and
   the deduped per-callable `ctx=[…]` — plus the diagnostics, sorted by `(code, message)`
   so the golden is pass-order-independent. A parse failure short-circuits to `PARSE-ERR`
   (malformed input belongs in the parser goldens). Five seed cases: `clean_relations`,
   `ctx_scope`, `inferred_index`, `errors_bundle`, `lints`.

## Milestones ahead (post-sema)

**M2 — SQL DDL codegen (`based gen sql`). ✅ done.** `based-codegen::sql::ddl` renders
`CheckedSchema` → MariaDB `CREATE TABLE`: columns (scalars, FK `<field>_id`, implicit
`id`), PK, `(unique)` constraints, declared `@index`es (relation cols resolved to FKs),
type mapping + no-FK-constraint rule recorded in decisions.md **D10**. IR enriched:
`MemberKind::Scalar` now carries `unique` + `default`. Tests: `based-codegen/tests/ddl.rs`;
commerce example generates clean DDL.
  - ~~*Deferred inside M2*: the inferred baseline index set.~~ ✅ **done with sema
    resume #3** (D15): DDL now appends the sema-inferred join-key indexes
    (`KEY inf_<table>_<cols>`), soft-delete column prepended (predicate-leading —
    MariaDB has no partial indexes), deduped against declared structure. Filter-path
    indexes deliberately stay out of DDL — they surface as `W0103` instead.
  - *Deferred*: per-field length tuning for `text` (no length primitive; D10 uses
    `VARCHAR(255)`); custom-PK FK type propagation is handled but untested for non-uuid keys.

**M3 — query/mutation SQL.**

*Read side (`sql::dml`) ✅ done.* Each `query` lowers to a parameterized SELECT
(`based gen sql` appends them after the DDL; tests: `based-codegen/tests/dml.rs`,
10 cases; commerce generates clean SELECTs). Delivered:
  - **Headline soft-delete injection** (soft-delete.md): tombstone predicate on the
    root table (`WHERE`) *and* every joined table (in its `ON`, so `LEFT JOIN` stays
    left). `@scope` (auth.md) rides the same path. Conventions recorded in **D11**.
  - Shape projection: bare local columns, `out = path` relation reaches (each hop a
    JOIN, deduped by path prefix, aliased `j_<prefix>`), `out = sql`…`` inline exprs.
    Bare-model return projects every stored column (FKs as `<field>_id`).
  - Filters: bare/inline same-name equality (relation param → FK col), per-param
    bindings (`-> edge`, `op col`), explicit block/inline `where`; bare bool → `= TRUE`.
  - Sort cascade (query `order` > model `@sort`) + keyset `id` tiebreaker; `page` →
    `LIMIT`/`OFFSET`; `with count` → a second live-row `COUNT(*)`.
  - **Named-filter calls in `where` are inlined** (D14 codegen twin): a `FilterCall`
    (or a bare atom naming a filter) substitutes its args through the filter body and
    lowers it against the call-site model, reusing the join/predicate resolver — so a
    relation-reaching filter body emits its joins too. Self-reference is guarded
    (`filter_stack`) with a visible `/* filter … recursion */` marker. Threaded through
    the write side as well (`Select` now carries the filter map). Tests: 3 new in
    `dml.rs` (13 total) + 1 in `mutations.rs` (9 total).
  - *Deferred inside M3 read*: nested shape sub-objects (`field { … }` — needs JSON
    aggregation / a second query; skipped in projection); `@scope` injection (design
    open — see decisions.md; `@tenant` removed, folded into `@scope`); keyset cursor
    comparison + opaque cursor encoding (runtime concern — base SELECT is ORDER+LIMIT).

*Write side (`sql::mutations`) ✅ done.* Each `mutation` body lowers to INSERT /
UPDATE / DELETE (`based gen sql` appends them after the queries; tests:
`based-codegen/tests/mutations.rs`, 8 cases; commerce `place_order` generates a clean
INSERT). Conventions recorded in **D12**. Delivered:
  - **Soft-delete rewrite is the headline** (soft-delete.md): `delete` on a
    `@soft_delete` model becomes the tombstone UPDATE, *never* a real DELETE;
    `restore` clears it (inverse); `hard delete` is the loud opt-out that does emit a
    real `DELETE`. Plain models get a plain `DELETE`.
  - **Injected guards**: the soft-delete live predicate + `@scope` ride into every
    UPDATE/DELETE `WHERE` so a write can't touch a tombstoned or out-of-scope row
    (restore skips the live predicate — it targets deleted rows — but keeps scope;
    hard delete skips the tombstone but keeps scope). Reuses the read-side join
    resolver, so a relation-reaching `where` lowers to MariaDB's multi-table
    `UPDATE m JOIN …` / `DELETE m FROM m JOIN …`.
  - **Engine columns**: app-generated `id` bound as `:id` on INSERT (D1, no SQL
    default; skipped if the caller sets its own `id`); `@created`/`@updated` set to
    `CURRENT_TIMESTAMP` on insert, `@updated` bumped on every UPDATE (incl. the soft
    delete/restore rewrites), all skipped when the caller assigns them explicitly.
  - **`tx`** renders its inner writes in order under one engine-owned transaction
    (principle 7 — the engine, not the emitted SQL, owns BEGIN/COMMIT).
  - **`^` tx back-references** (`user = ^.id`) now lower (D16, sema resume #6): sibling
    creates in a `tx` get distinct id binds (`:id_<step>`) and a back-reference reads
    the immediately preceding create.
  - **Declared-shape re-select** (D12): a create-returning mutation now emits a trailing
    `SELECT` reading the created row back in its declared shape (`ret_select`, keyed on
    `:result_id`), reusing the read side's `project_return`. The runtime runs it inside the
    write tx (M6 write). A pure update/delete still emits none (deferred).
  - *Deferred inside M3 write*: required-field enforcement on `create` is now a sema error
    (resume #7, `E0146`), so a clean schema never reaches codegen with unassigned required
    columns; raw write statements have no attached model so `{table}`/`{id}` interpolation
    has no root to bind.

**M4 — client codegen (`based gen client`). ✅ done.** `based-codegen::client` renders the
`CheckedSchema` → a typed Rust client module (manifest `client` target; Rust first + default).
Conventions recorded in **D13**. Tests: `based-codegen/tests/client.rs` (10 cases); the commerce
example generates a module that compiles clean against `serde`/`serde_json`. Delivered:
  - **One route per callable** (`POST /q/<name>` / `POST /m/<name>`), each a `const` + a
    `Client<T: Transport>` method that posts the input struct and decodes the output.
  - **Input struct** per signature: explicit param annotations map through (model type → `Uuid` FK,
    D1); untyped params infer from the mapped column (`-> edge`/same-name relation → `Uuid`, `op col`/
    same-name scalar → its type); defaulted/optional params → `Option<T>`. `$ctx` is never an input.
  - **Output type** from `-> Output`: a shape → a struct projecting its body (relation reach terminal →
    `Uuid`); a bare model / `full` → every stored column (FKs as `Uuid`); shared shape → one struct.
    **Return wrapper**: paginated → `Page<T>` (`{ rows, cursor }` envelope), `list`/many → `Vec<T>`,
    `get` → `Option<T>`; mutation → the single `T`.
  - **Type aliases** mirror the DDL side (`Uuid`/`Timestamp`/`Date` = `String`, `Json` =
    `serde_json::Value`); Rust-keyword field names are `r#`-escaped.
  - **Transport is abstract** — the generated `Client<T>` delegates to a `Transport` trait; the runtime
    (M-runtime) supplies the concrete HTTP/driver binding. Codegen emits the typed surface only.
  - *Deferred inside M4*: nested shape sub-objects skipped in the output struct (need JSON aggregation,
    same as M3 read); a `sql`…`` shape field → `Json` (no static type); the keyset cursor is an opaque
    `Option<String>` (its encoding is a runtime concern). ~~Polyglot clients are **not** a
    per-language emitter — they come from an **OpenAPI spec emitter**.~~ ✅ **delivered
    (`based gen openapi`, D24):** one OpenAPI 3.1 contract off the *same* `CheckedSchema` +
    AST + type resolver the Rust client uses, so `openapi-generator` produces TS/Python/Go/etc.
    from one artifact (D23's decision, now built). The Rust client stays hand-emitted (it's the
    in-process `Transport` path, tighter than a generated HTTP stub); `ClientTarget` still
    branches only for the emitters we hand-write (Rust today), not for every wire language.

**M5 — LSP (show-don't-write, principle 8). ✅ done.** Engine-derived facts are
*shown* in the editor, never forced into source. Two layers:

- **`based-facts`** — the pure core. `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>`
  emits span-anchored `Fact { span, kind, label, detail }`. Two kinds today:
  `InferredInverse` (a `[]` back-edge whose paired forward field sema inferred —
  shown only when the author didn't write `(Model.field)`, so it's genuinely a
  not-in-source fact; the `decls` arg is consulted only for that distinction) and
  `InferredIndex` (a join-key baseline index the DDL will emit; the label/columns
  reproduce `sql::ddl`'s `inf_<table>_<cols>` naming + soft-delete-leading order so
  the shown fact matches the generated DDL exactly), plus two callable-level kinds:
  `CtxRequirement` (the deduped `$ctx.<field>: type` bag a query/mutation silently
  requires — typed by inference per callable, D4/D5; the label mirrors the sema
  conformance rendering, `field: -> Model` / `field: <prim>`, and the client sends
  exactly these) and `ResolvedQuery` (a query's inferred verb/target/cardinality/
  pagination — none of it in the signature, queries.md). Both anchor at the callable
  declaration; the LSP places them at the header line's end. Output is span-sorted
  for stable goldens. Tests: `based-facts/tests/facts.rs` (8 cases); commerce
  surfaces the `Order.items <- OrderItem via order` inverse, the `my_org_orders`
  `ctx requires [org: -> Org]`, and every query's resolved shape.
- **`based-lsp`** — the transport. A tower-lsp/tokio server over stdio. On
  open/change/save it recompiles the project (the same discover→parse→check front end
  as the CLI, with unsaved buffers overlaid on disk by canonical path) into a
  `Snapshot` (sources + per-file `LineIndex` + facts + diagnostics), then serves:
  **diagnostics** (every parse/sema error + lint, mapped span→range, republished for
  all files so fixes clear), **inlay hints** (each fact placed next to its
  declaration — inverse after the field, index at the model header line — with the
  `detail` as tooltip), and **hover** (the fuller "why" for any fact whose span
  covers the cursor). `LineIndex` does faithful UTF-16 position mapping (LSP's
  default). Tests: `based-lsp/src/compile.rs` unit tests (position round-trips incl.
  multibyte; `compile` over commerce). Smoke-tested end-to-end over the JSON-RPC wire.
- **`based facts [--json]`** — the same core exposed on the CLI (`file:line:col  kind
  label` + a `= note` "why" line, or a hand-rolled deterministic JSON array).
  *Deferred inside M5* (what's shipped is the principle-8 core — derived facts +
  diagnostics; the rest is sequenced MVP-first):
  - Incremental (range) document sync — today FULL-sync recompiles the whole project
    per edit (fine at this scale).
  - ~~Surfacing `$ctx` requirements + the resolved query shape as facts.~~ ✅ **done.**
    Two new `FactKind`s in `based-facts` (`CtxRequirement`, `ResolvedQuery`) read
    straight off the IR (`RQuery`/`RMutation.ctx_requires`, `RQuery.verb/target/
    many/paginated`) — no new resolution. Both surface via `based facts` and the LSP
    (inlay + hover) with no LSP-side logic beyond one inlay-placement arm. Tests: 3
    new in `facts.rs` (8 total).
  - **VS Code client extension** — the next milestone for the editor line. The server
    already speaks standard LSP, so any client attaches; an actual packaged extension
    is what turns this into something a user runs. Wanted *before* the IDE-ergonomics
    features below, because an MVP a human can use beats a smarter headless server.
  - **Go-to-definition / completion / rename — planned, needed before v1, deferred.**
    These are general IDE ergonomics, not derived-fact surfacing, so principle 8
    neither requires nor forbids them — they're an ordinary product call, sequenced
    after the VS Code client. They also need infra the server lacks today: a
    position→symbol resolution layer (offset → the resolved thing here + all its
    reference sites, cross-file), which rename in particular depends on. Land the
    client first, then build this layer and these features on top.

**M6 — runtime (`based-runtime`). 🚧 read + write path done.** The engine that turns
a wire request into a bound, executable statement and shapes the result. Architecture:
**in-process** (D18) — the runtime links `based-sema` + `based-codegen`, holds the
same `CheckedSchema` the compiler produced, and reuses codegen's *one* query and
mutation lowering (`sql::lower_queries` / `sql::lower_mutations`) rather than
re-deriving SQL or parsing a serialized artifact. So the executed SQL and its bind
surface can never drift from `based gen sql` (principle 4). Tests:
`based-runtime/tests/query.rs` (12) + `mutation.rs` (8) + `load.rs` (commerce, incl.
`place_order`) + the scanner unit tests (6); the whole request→JSON path runs against
a `MockDb`, no live DB.

*Read side (this slice) — delivered:*
  - **`Compiled::load`** runs the front end (discover→parse→check, bail on any error
    — a dirty schema never reaches the runtime) then lowers every query, keyed by
    name for O(1) dispatch. `from_checked` is the disk-free seam tests use.
  - **`plan_query`** (`plan.rs`) — the core. Validates each arg against the signature
    (required / `(default)` applied / family-coerced from JSON, calling.md #3), threads
    the per-callable `$ctx` requirement bag (D4/D5 — `:ctx_<field>` binds from request
    context, *not* args; a missing one is `MissingCtx`), and binds every `:name`
    placeholder to positional `?` in SQL order. Picks the response `Envelope` from the
    inferred verb/pagination: `get`→`One`, `list`→`Many`, paginated `list`→`Page`.
  - **Named→positional binding** (`scan.rs`) — a quote-aware scanner rewrites `:name`
    →`?`, pulling values from one environment assembled from the validated inputs. The
    *names* are unambiguous given the schema (`:<param>` / `:ctx_<field>` / `:offset`),
    so no parallel bind manifest is kept — the SQL is the one source of the bind
    surface (P4). Skips colons inside `'…'`/`"…"`/`` `…` `` literals and `::`.
  - **Input coercion** (`value.rs`) — `SqlValue` is the driver-neutral bound value;
    coercion is family-aware (an `int` param rejects a JSON string *before* SQL).
    Families are coarse, matching sema's `=`-operand families (D1): `uuid`/`timestamp`/
    `date`/`Id` ride as text. An untyped param is shape-coerced (`Family::Any`).
  - **`run_query` + `Db`** (`run.rs`) — execution goes through the abstract `Db` trait
    (the runtime's twin of the client's abstract `Transport`); a `MockDb` returns canned
    rows. Row shaping realizes the envelope: `get`→object/`null`, `list`→array,
    paginated→`{ rows, cursor }` (+`total` for `with count`).
  - *Deferred inside M6 read*: the keyset **cursor** rides as `null` (encoding is a
    driver concern, pagination.md); strict per-column typing of *untyped* params (the
    mapped-column family isn't re-derived — the typed client already sends the right
    shape); the offset value arrives as an `offset` arg (defaulting to 0).

*Write side (this slice) — delivered:*
  - **Structured mutation lowering** (`sql::lower_mutations`, codegen) — the write twin
    of `lower_queries`. Each mutation lowers to a flat `Vec<LoweredWrite>` (a `tx` is
    flattened — the whole body already runs under one transaction), each carrying
    header-free SQL, the target model, and the bind name of the engine `id` a `create`
    generates (`gen_id`). The text emitter (`based gen sql`) now frames this one
    lowering with comment headers, so the emitted and executed writes can't drift (P4).
  - **`plan_mutation`** (`plan.rs`) — mirrors `plan_query`: validates args + `$ctx`
    (reusing `bind_param`/`bind_ctx`), then generates each `create`'s engine `id`
    (`IdGen`, D1) into the value environment *before* binding — so a `^.id` back-ref,
    which lowered to the prior create's `:id_<step>`, resolves to the same value the
    INSERT used. Binds every write to positional `?` in SQL order. Records the
    return-model create's id as `result_id` (the row the response identifies).
  - **`IdGen` seam** (`id.rs`) — the write twin of the read path's `MockDb`: a trait so
    prod supplies uuids (with the driver slice) and tests supply the deterministic
    `SeqIdGen` (`id-0`, `id-1`, …), making a planned INSERT's bound id predictable.
  - **`run_mutation` + `Db` writes** (`run.rs`) — the `Db` trait grew `execute` +
    `begin`/`commit`/`rollback` (defaulted, so a read-only `Db` is unaffected).
    `run_mutation` executes every write in order between one `begin`/`commit`
    (principle 7 — the engine owns the transaction, not the emitted SQL) and returns
    the write response.
  - **Declared-shape re-select** (D12, this slice): a mutation that **creates** its return
    row now reads it back in its declared shape after the writes. Codegen (`sql::mutations`)
    emits a trailing `ret_select` — `SELECT <return shape> FROM <return model> WHERE id =
    :result_id [AND <live> AND <scope>]` — reusing the read side's `project_return` so it
    can't drift from a `get` (P4); `plan_mutation` binds `:result_id` to that create's engine
    id and `run_mutation` runs the re-select **inside** the write tx (read-your-writes), and
    its single shaped row *is* the response — matching the client's decoded output type.
    `tests/embed.rs` now round-trips the verbatim generated `place_order` into a typed
    `OrderCard`. Chose re-select over MariaDB `INSERT … RETURNING`: dialect-portable, reuses
    the one projector, handles the shape's relation joins uniformly.
  - *Deferred inside M6 write*: a **pure update/delete** that declares a return shape still
    responds `{ id }`/`{}` — it has no engine-generated id to key a re-select on (its
    re-select would key off the write `where`, cardinality-ambiguous); a `create` whose `id`
    the caller sets (`gen_id: None`) is not surfaced in `result_id`; the concrete uuid `IdGen`
    lands with the driver.

*Dispatch + driver core (this slice) — delivered (D20):*
  - **Enterprise-scale architecture decided (D20):** sync + bounded connection pools,
    horizontal **scale-out** for load (shards + app instances behind an LB), **single-shard
    per request** (no scatter-gather → a `tx` is one shard, no distributed transaction;
    a down shard fails only its own traffic). Async was weighed and rejected: the DB
    connection pool is the real ceiling and is bounded in *both* models, so async's
    idle-socket win doesn't apply to a bounded-pool, DB-bound, LB-fronted RPC service —
    while its complexity/cancellation cost is at odds with "very dependable, low complexity."
  - **Fallible `Db`** — every method returns `Result<_, DbError>`; a mutation rolls back
    on any write failure (all-or-nothing, principle 7). `run_query`/`run_mutation` return
    `RunError` = `Plan(PlanError)` | `Db(DbError)`.
  - **`serve::dispatch`** (`serve.rs`) — the wire core, pure and mock-tested (no socket):
    routes `POST /q|m/<name>` (prefix authoritative, no cross-dispatch), builds the
    `Request` (`$ctx` supplied out-of-band, never the body — auth.md/D7), runs it, and
    maps every outcome to a `WireResponse`: 200 + shaped JSON; PlanError → 400/404/500;
    DbError → retryable **503**. Tests: `based-runtime/tests/serve.rs` (8).
  - **Concrete `MariaDb` driver** (`driver.rs`, feature `mariadb`) — a real `Db` over one
    pooled `mysql`-crate connection (pure-Rust driver + its hardened pool, principle 7,
    TLS/compression off to avoid a system OpenSSL dep). `SqlValue`↔`mysql::Value` mapping
    is pure + unit-tested; connecting/executing is compile-verified (no live DB here).
  - **`ShardRouter`** — the scale-out seam: one bounded pool per physical shard, routing
    each request to exactly one shard via a **stable FNV logical-shard hash** (fixed
    `LOGICAL_SHARDS=4096` space, `logical→physical` assignment) so adding a shard moves
    whole logical shards without rehashing keys (Vitess/Citus model). `single(url)` for
    the N=1 common case; the router is the seam so splitting later is config, not code.
    Key extraction is left pluggable + **decoupled from D19** (`@scope`/tenant still OPEN),
    so the shard key is not yet bound to a `$ctx.<field>`.

*HTTP listener (`based serve`) — delivered (D21):*
  - **`based-runtime::http`** (feature `serve`) — the thin socket edge over `serve::dispatch`.
    A **sync bounded worker-thread pool** over the bounded connection pool (D20): N workers
    share one blocking `tiny_http::Server` (hardened lib, principle 7), each looping
    `recv → decode → dispatch → respond`. `based serve <root>` (CLI) loads the checked schema,
    builds the `ShardRouter`, and runs it (`--listen`, `--database-url` × shards / `BASED_DATABASE_URL`,
    `--workers`, `--pool-{min,max}`, `--shard-key-field`).
  - **`$ctx` from headers, never the body** (auth.md/D7): a pluggable `ContextSource` derives
    `$ctx` + the shard key from request headers; the default `TrustedHeaderContext` reads a
    pre-authenticated `X-Based-Context` (JSON) an upstream auth proxy sets. Non-object → 400.
  - **Pre-checkout guard** (`serve::preflight`): a non-POST / unroutable request is rejected
    *before* a pooled connection is borrowed; `dispatch` runs the same guard (one source of truth).
  - **Production `UuidGen`** (v4, D1), built fresh per request (id state is per-request, never
    shared across worker threads).
  - **Driver-neutral edge (multi-dialect readiness, D21):** the listener depends only on the new
    `Backend` seam (`run::Backend` — a connection source yielding a boxed `Db`), never a concrete
    driver, so a future Postgres/MySQL/SQLite backend drops in without touching `based serve`. See
    D21 for the full readiness story (the `Dialect` codegen seam + the one `?`-vs-`$n` scanner
    coupling to fix when a non-`?` engine lands).
  - Tests: `based-runtime/tests/http.rs` (7 end-to-end over a real loopback socket — routing,
    header-`$ctx`, body decode, uuid write response, 400/404 edges) + 5 `http` unit tests (header
    view + `TrustedHeaderContext`). The pure `serve.rs` dispatch tests (8) still cover the core.

*Container story (`based serve` as a deployable container) — delivered (D26):*
  - **Health/readiness probes** (`http`, feature `serve`): `GET /healthz` = liveness (always `200`
    while serving, **touches no DB** — a DB outage drains, not restarts) and `GET /readyz` =
    readiness (`200` only when not draining *and* `Backend::ping` succeeds; `503` `draining` /
    `not_ready` otherwise). Both are unauthenticated GETs answered *before* routing, so the RPC wire's
    POST-only rule is unchanged. `Backend::ping` is the readiness seam (defaulted; `ShardRouter` probes
    **every** shard with `SELECT 1`).
  - **Graceful shutdown** via `Handle::shutdown` (from the new `serve_with_handle`; `serve` is now a
    thin no-handle wrapper): flips a shared *draining* flag so readiness fails **first** (the LB drains
    this instance), then workers finish their **in-flight** request and exit (`recv_timeout` poll — no
    request is ever cut off), and the serve call returns so the process exits cleanly. The
    SIGTERM/SIGINT→drain wiring lives in the **CLI** (`based serve`, via the `ctrlc` crate — the runtime
    library stays signal-free); `based serve` now also logs the probe routes on startup.
  - Tests: 4 new in `based-runtime/tests/http.rs` (12 total) — `/healthz` OK & DB-free, `/readyz` OK,
    `/readyz` 503 when the backend is down (liveness still OK), and end-to-end graceful drain (readiness
    flips to 503, the serve thread returns after draining).

*SQLite backend + real integration tests (D27) — delivered:*
  - **`based-runtime::sqlite`** (feature `sqlite`) — the infra-free concrete `Db`/`Backend`, the
    twin of `driver::MariaDb`/`ShardRouter`. `SqliteDb` runs the runtime's real read/write SQL over
    one bundled-SQLite connection (`rusqlite`, no system dependency, principle 7); `SqlValue`↔
    `rusqlite::Value` mapping is pure + unit-tested (bool→0/1, json→text, blob→hex, mirroring
    `from_mysql`). `SqliteBackend` is the `Backend`: one shared connection behind a `Mutex` (so an
    in-memory DB stays coherent across checkouts — the property that makes it a real test engine),
    no shards (ignores the shard key), `ping` = `SELECT 1`. SQLite binds positional `?` like MariaDB,
    so **no dialect-aware scanner change** (D21's `?`-vs-`$n` note is Postgres-only). **SQLite DDL
    codegen `sqlite` (D28):** `Dialect::Sqlite` — the `Dialect` enum's first second variant — makes
    `based gen sql` emit SQLite-shaped DDL (TEXT/INTEGER type map mirroring `SqliteDb`; declared +
    inferred indexes as separate `CREATE INDEX` statements; bool defaults as `0`/`1`); DML/mutation
    SQL is already dialect-portable. The D27 integration test now creates its tables from this
    *generated* DDL, so the whole `based gen sql` artifact (DDL + DML) is proven to execute.
  - **Real end-to-end integration** (`tests/sqlite_integration.rs`, 6 tests) — loads the *actual*
    commerce schema (`Compiled::load`) and drives real requests through `serve::dispatch` against a
    live `SqliteDb`, executing the *verbatim* codegen-lowered SQL (`based gen sql`) — the first tests
    that prove the emitted SQL runs, not just that binding is right (every other runtime test uses
    `MockDb`). Covers: a `get` (join + project) + its miss→`null`, a `$ctx`-scoped `list` (scope
    predicate actually filters), the `place_order` write (INSERT + declared-shape re-select under one
    tx, read-your-writes verified by a follow-up read), a boundary `400`, and `Backend::ping`.
  - ~~*Deferred inside D27*: **SQLite DDL codegen**~~ ✅ **done (D28).** `Dialect::Sqlite` now makes
    `based gen sql` emit SQLite DDL (TEXT/INTEGER type map; indexes as separate `CREATE INDEX`; bool
    defaults `0`/`1`), and the integration test creates its tables from that *generated* DDL rather than
    a hand-shaped copy. A `SqliteBackend` *shard router* is unneeded (SQLite doesn't shard).

*Not started (next slices):*
  - ~~**Additional dialects (Postgres / MySQL)**~~ **Postgres codegen + scanner ✅ done (D29).**
    `Dialect::Postgres` is the enum's third variant: `ddl`/`dml`/`mutations` all branch (double-quoted
    identifiers via one `Dialect::quote`/`qcol` seam, native type map incl. `TIMESTAMPTZ`/`JSONB`,
    `CREATE INDEX` indexes, `has` → `@>`, and the `FROM`/`USING` multi-table UPDATE/DELETE restructure +
    bare-column `SET`), and the named→positional scanner is now dialect-aware (`?` for
    MySQL/MariaDB/SQLite, `$n` for Postgres — the one coupling D21 flagged). `Compiled` carries the
    `Dialect` (from the manifest) and threads it through binding, so a Postgres schema lowers *and* binds
    for Postgres. Commerce emits clean Postgres SQL. **Still outstanding on the dialect line:** the
    concrete `postgres` `Db`/`Backend` **driver** (deferred to the live-DB slice — needs a real server,
    same status as `MariaDb`'s compile-verified connect/exec). MySQL stays folded into `MariaDb` (a fork;
    the emitted SQL is MySQL-8-compatible), so no separate variant is warranted.
  - **Live-DB integration + the Postgres driver** — exercise `MariaDb` against a real MariaDB (the
    connect/exec paths only compile-verified today): typed JSON reconstruction for `JSON` columns,
    statement timeouts, deadlock-retry, pool-exhaustion → 503 under load. `Backend::ping` (D26) is
    compile-verified only until this lands. **The concrete `postgres` `Db`/`Backend` driver belongs
    here too** — Postgres *codegen* + the `$n` scanner are done (D29), but running the emitted SQL
    needs a real server (an infra-free SQLite-style in-memory test isn't available for Postgres), so
    the driver is the live-DB slice's job, over the same `Db`/`Backend` seam the HTTP edge already uses.
  - **Container packaging** — a Dockerfile / image is the last mile of the container story (the
    health/readiness + graceful-shutdown *behaviour* is done, D26; packaging it is orthogonal). A
    shutdown grace deadline (force-exit after N seconds) is deferred with it.
  - ~~**Idempotency for write retries**~~ ✅ **done (D25).** A keyed mutation runs its write body
    **at most once** per `(callable, key)`: a retry replays the first attempt's stored response
    instead of double-inserting (the app-side `id`-gen hazard, D1/D20). The key is out-of-band
    request metadata (the `Idempotency-Key` header — **not** the body, **not** a `$ctx.<field>`;
    it is engine infra, not app data). `IdempotencyStore` is the seam (the `Db`/`IdGen` twin);
    `MemStore` is the in-process impl (single-instance-correct, testable with no infra), `NoStore`
    the no-op so there is one dispatch path (P4). `run_mutation` consults it *after* planning (a bad
    request never consumes a key); a concurrent in-flight duplicate is a retryable `409`
    (`RunError::Conflict`). Wired through the HTTP edge (shared store across the worker pool),
    `embed::Engine::call_with_key`, and `dispatch`. Tests: 4 store unit + 4 in `serve.rs` (dedupe /
    retryable-on-failure / no-slot-on-bad-request) + 1 socket end-to-end. *Deferred:* a shared/durable
    store for multi-instance dedupe (needs live infra — same trait), key TTL/eviction, and rejecting a
    replayed key carrying *different* args (today the key alone is authoritative, the Stripe default).

*Two front doors — embed as a library (Rust) OR run as a container (any lang). Planned,
mostly-glue:* the engine is already **in-process by design** (D18) and `serve::dispatch`
is transport-agnostic (method/path/args/`$ctx` → `WireResponse`, no socket), and the
generated client is generic over an abstract `Transport` trait (`call(route, input) ->
Result<O>`, M4) whose own doc reserves it for "the runtime's client". So both doors are
the *same* engine; what's missing is connective tissue, not architecture. **Key insight
that orders the effort:** the per-call cost is the DB round-trip (0.2–5 ms, D20) and, over
the wire, the loopback TCP + HTTP framing — JSON ser/deser of a small arg object is
negligible next to those. So the win is *dropping the socket*, not *dropping JSON*; effort
should chase the former.
  - ~~**Tier 1 — in-process `Transport` (recommended, ~zero engine change).**~~ ✅ **done
    (D22).** `based-runtime::embed::Engine` (`Compiled` + one `Db` + `IdGen`, held behind a
    `RefCell` so a call needs only `&self`) runs a callable through `serve::dispatch` with no
    socket, returning the identical `WireResponse` the HTTP edge does — same plan → run →
    shape path (P4). The client's `Transport` trait is defined *by* the generated code, so
    by the orphan rule the ~10-line bridge (`serialize → engine.call → decode 200 body; non-200
    → ClientError`) lives in the embedding crate — shown in `Engine`'s docs and exercised by
    the worked example `tests/embed.rs` (the *verbatim* `based gen client` output over a
    `MockDb`: typed `order_by_id`/`orders_in_org`/`my_org_orders` round-trips, `$ctx` supplied
    straight in — no header dance, and the write `place_order` now decodes into a typed
    `OrderCard` via the declared-shape re-select, D12). Unlocks one binary (no sidecar), steadier latency,
    `MockDb` end-to-end tests, and the path toward **app-owned transactions** (compose several
    callables in one unit-of-work over a shared connection — inexpressible on stateless HTTP RPC;
    the real long-term prize). Concurrency: one connection ⇒ one thread at a time; a pooled embed
    routes through the `Backend` seam (build a short-lived `Engine` per checked-out connection).
  - **Tier 2 — embed ergonomics.** A small `Engine` convenience wrapper over
    `Compiled` + the caller's own `Db`/pool (the `Db` seam already lets an app plug an
    existing pool — a feature, not a gap); document the in-process `$ctx` path (supplied
    straight to `Request::new`, cleaner than the header dance the HTTP edge needs, D21).
  - **Tier 3 — JSON-free typed path: explicitly NOT planned.** Binding the input struct
    straight to `SqlValue` (no `serde_json` in the middle) is a real codegen effort whose
    payoff is nanoseconds against a millisecond DB call — skip unless profiling ever
    demands it. Recorded here so the "purity" idea isn't re-litigated.
  - ~~**Gates the *container* door for non-Rust langs (orthogonal to the above) — via OpenAPI,
    not per-language emitters (D23).**~~ ✅ **the emitter is done (`based gen openapi`, D24).**
    A single OpenAPI 3.1 document off the same `CheckedSchema` — one `POST /q|m/<name>` path +
    input/output `components.schemas` per callable, the `Page`/`{ id }`/error envelopes, and
    `$ctx` modelled as the `X-Based-Context` header — so `openapi-generator` turns it into a
    client in any language. gRPC was rejected for this (D23): its perf win is void here (D20 —
    DB-bound, small args, unary CRUD, no streaming), it re-imports the async/heavy stack D20
    avoided, and it penalizes the primary web/TS caller (needs grpc-web + a proxy); plain
    JSON/HTTP is the boring, browser-native, LB/gateway-frontable surface `serve::dispatch`
    already serves. ~~*Still wanted for the standalone container story:* health/readiness +
    graceful shutdown~~ ✅ **done (D26):** `GET /healthz` (liveness, DB-free) + `GET /readyz`
    (readiness via `Backend::ping`) + graceful drain on SIGTERM/SIGINT (`Handle::shutdown` /
    `serve_with_handle`, wired in the CLI via `ctrlc`; in-flight requests always finish).
    *Still wanted:* a **container image / Dockerfile** (packaging, orthogonal to the behaviour)
    and the **live-DB hardening** above (not production-real until that lands).

## Conventions

- Rust workspace, edition 2021, rust-version 1.85. `cargo test` / `cargo clippy` /
  `cargo fmt --check` must stay clean (stock rustfmt, no config).
- Diagnostics carry spans (`FileId` + byte range); `based-cli/src/render.rs` frames
  them rustc-style. New checks → new stable code in `ir::code` + a note when the fix
  isn't obvious from the message.
- Audience is LLMs + reviewers: optimize tokens-to-comprehend, readable > terse
  (CLAUDE.md). Match surrounding comment density.
- `spec/principles.md` are the tiebreakers, in order. `spec/decisions.md` (D1–D9)
  resolves anything the prose left open.
