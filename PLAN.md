# PLAN.md — build-out roadmap

Working notes for whoever picks this up next. Records what's **done**, what's
**deferred** (with enough context to resume without re-deriving), and the
**remaining milestones**. Spec is truth for *what* the language is; this is truth
for *where the implementation stands*.

## Pipeline (data flow)

```
*.bsl ──manifest::discover──▶ files
      ──parser::parse_file──▶ [Decl]           (per file; recovers at decl boundary)
      ──sema::check─────────▶ CheckedSchema + [Diagnostic]
      ──codegen::sql::ddl───▶ SQL DDL          (M2 ✅)
      ──codegen::sql::dml───▶ query SELECTs    (M3 read side ✅)
      ──codegen::sql::mutations─▶ INSERT/UPDATE/DELETE  (M3 write side ✅)
      ──codegen::client─────▶ typed Rust client (M4 ✅)
      ──facts───────────────▶ engine-derived facts    (M5 ✅)
                              └─ based-lsp ──▶ editor inlay hints + hover + diagnostics
      ──runtime::plan/run───▶ bound positional statement + shaped JSON  (M6 read+write ✅)
```

`based check` wires discover → parse → sema → render. `based gen sql [--out]` runs the
same front end (`load_checked` in based-cli), then lowers the `CheckedSchema` to DDL,
then appends the query SELECT templates (`sql::dml`) and the mutation write templates
(`sql::mutations`), both reading the AST alongside the IR. `based gen client [--out]`
runs the same front end, then lowers to a typed Rust client module (`client`). All bail
unless every file parses *and* checks clean (codegen assumes a clean schema).

## Crate status

| crate | state | notes |
|-------|-------|-------|
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). Missing: schema-version. (`$ctx` is inferred in sema, not declared here — D4.) |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. |
| **based-sema** | ✅ **this milestone** | resolution + checks + lints + `CheckedSchema` IR. Details below. |
| based-cli | ✅ works | `based check` + `based gen sql` (DDL + query SELECTs + mutations) + `based gen client` (typed Rust) + `based facts [--json]` (derived facts, M5). |
| **based-codegen** | ✅ **M2 (DDL) + M3 (read+write) + M4 (client)** | `sql::ddl` → `CREATE TABLE`; `sql::dml` → query SELECTs (`lower_queries` seam); `sql::mutations` → INSERT/UPDATE/DELETE (soft-delete rewrite + scope injection; `lower_mutations` seam feeds both the text emitter and the runtime); `client` → typed Rust client (inputs/outputs/routes). |
| **based-facts** | ✅ **M5** | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>`: the "show, don't write" facts — inferred inverse pairings, join-key indexes, per-callable `$ctx` requirement bags, and each query's resolved shape (verb/target/cardinality/pagination) — span-anchored. Golden/unit-tested; consumed by the CLI + LSP. |
| **based-lsp** | ✅ **M5** | tower-lsp server. Recompiles on edit (discover→parse→check, unsaved buffers overlaid on disk), publishes diagnostics + inlay hints + hover from `based-facts`. |
| **based-runtime** | 🚧 **M6 (read + write path)** | in-process engine (D18). `Compiled::load` reuses the front end + codegen's query *and* mutation lowering; `plan_query`/`plan_mutation` validate args/`$ctx`, bind `:name`→positional `?`, pick the response envelope (reads) / generate engine ids + thread `^` back-refs (writes); `run_query` shapes rows, `run_mutation` executes writes under one `begin`/`commit` — all via an abstract `Db` (mock-tested). **Concrete MariaDB driver + HTTP server not started.** |

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
  - *Deferred inside M3 write*:
    returning the declared shape after a write (RETURNING vs. re-select) — a runtime
    concern, no trailing SELECT emitted; required-field enforcement on `create`
    is now a sema error (resume #7, `E0146`), so a clean schema never reaches
    codegen with unassigned required columns; raw write statements have no attached model so `{table}`/`{id}`
    interpolation has no root to bind.

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
    `Option<String>` (its encoding is a runtime concern). A second client target (e.g. TypeScript) is
    the natural next emitter — the `ClientTarget` enum already branches for it.

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
  - *Deferred inside M6 write*: the **write response is the created row's engine `id`**
    (`{ "id": … }`) or `{}` when nothing is created — the declared-shape re-select
    (RETURNING vs. re-select, D12) is still deferred, so the response does not yet
    match the client's decoded output type; a `create` whose `id` the caller sets
    (`gen_id: None`) is not surfaced in `result_id`; the concrete uuid `IdGen` lands
    with the driver.

*Not started (next slices):*
  - **Concrete MariaDB driver** — a real `Db` impl (reuse a hardened driver, principle
    7: `mysql_async`/`sqlx`) mapping `SqlValue`→its bind form and rows back to JSON.
  - **HTTP server** — the `POST /q/<name>` / `POST /m/<name>` wire surface (calling.md):
    decode JSON body + context → `Request` → `run_query`/`run_mutation` → JSON
    response. `based serve`.

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
