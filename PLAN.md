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
| based-cli | ✅ works | `based check` + `based gen sql` (DDL + query SELECTs + mutations) + `based gen client` (typed Rust). |
| **based-codegen** | ✅ **M2 (DDL) + M3 (read+write) + M4 (client)** | `sql::ddl` → `CREATE TABLE`; `sql::dml` → query SELECTs; `sql::mutations` → INSERT/UPDATE/DELETE (soft-delete rewrite + scope injection); `client` → typed Rust client (inputs/outputs/routes). |
| runtime | ❌ not started | see Milestones. |

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
  against the immediately preceding `create`; `E0170` outside a tx / no prior create).
- Implicit `id: Id` (D2); a model that declares its own `id` keeps it.
- Decorators: `@soft_delete` (covered-subset type check → `SoftMode`), `@created`/
  `@updated` (timestamp role), `@tenant`, `@scope` (predicate, `$ctx`-only), `@sort`
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
- Index inference + lints (indexing.md, D15, `indexes.rs`): per-query access
  patterns (eq/range/sort off the conjunctive spine, params + `@scope` + call-site
  filter bodies included) vs. available indexes → `W0103` missing-index (satisfied
  by `@index` or the `unindexed(max_rows: N)` / `unindexed(unsafe)` query clause,
  `W0105` when that annotation goes stale); pooled usage → `W0104` useless-index.
  Traversed inverse edges seed `RModel.inferred_indexes` (join-key baseline, DDL
  emits them `inf_`-prefixed, soft-delete predicate-leading).

**Diagnostic codes** live in `ir::code` (E01xx errors, W01xx lints). Parser owns
E0001/E0002, manifest E001x. Codes are stable — grep `ir.rs` for the registry.

**`CheckedSchema`** (the codegen seed): `models: Vec<RModel>` (fully resolved:
table name, members with kind Scalar/Forward/Inverse, soft_delete mode, sort,
scope, tenant, created/updated, indexes, unique_cols), plus resolved summaries
`shapes/queries/mutations/filters` and a `model_index` map. Codegen reads this
alongside the AST (`RQuery` carries inferred verb/target/many/paginated that are
*not* in the AST).

Tests: `crates/based-sema/tests/check.rs` (72 cases, positive + negative, keyed on
diagnostic codes). Commerce example (`spec/examples/commerce`) checks clean
(including a `$ctx.org` query whose context is inferred with zero config, D4/D5).

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
   `(unique)` constraint always flagged). *Still deferred*: mutation-`where`
   patterns feeding W0103; composite-prefix matching; prod-stats floors +
   `max_rows` re-checking; the `unsafe` audit listing; LSP surface (M5).
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
5. **Relation `on:` custom joins.** The predicate uses table-qualified names
   (`orders.user_ref = users.legacy_id`) which the single-model path resolver
   can't handle; currently accepted unchecked. Needs a two-table resolution scope.
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
   multi-level `^^`; back-ref *type* agreement with the assigned column.
7. **create/required-field enforcement.** `create` currently only checks that named
   columns exist; it doesn't verify all non-optional, non-defaulted columns are
   assigned.
8. **Sema conformance goldens.** Parser has `tests/conformance/`; add a sema golden
   harness (schema → resolved summary + diagnostics) mirroring that pattern.

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
    aggregation / a second query; skipped in projection); `@tenant` injection
    (semantics unspecified vs. `@scope`); keyset cursor comparison + opaque cursor
    encoding (runtime concern — base SELECT is ORDER+LIMIT).

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
    (sema resume #7) — an INSERT omits unassigned non-optional columns rather than
    erroring; raw write statements have no attached model so `{table}`/`{id}`
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

**M5 — LSP (show-don't-write, principle 8).** Surface engine-derived facts in the
editor: inferred inverse names, inferred indexes — never forced into source.

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
