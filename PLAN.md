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
      ──codegen::sql::mutations─▶ INSERT/UPDATE/DELETE  (M3 write side ✅; client TODO)
```

`based check` wires discover → parse → sema → render. `based gen sql [--out]` runs the
same front end (`load_checked` in based-cli), then lowers the `CheckedSchema` to DDL,
then appends the query SELECT templates (`sql::dml`) and the mutation write templates
(`sql::mutations`), both reading the AST alongside the IR. Both bail unless every file
parses *and* checks clean (codegen assumes a clean schema).

## Crate status

| crate | state | notes |
|-------|-------|-------|
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). Missing: `$ctx` type, schema-version. |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. |
| **based-sema** | ✅ **this milestone** | resolution + checks + lints + `CheckedSchema` IR. Details below. |
| based-cli | ✅ works | `based check` + `based gen sql` (DDL + query SELECTs + mutations). `gen client` is TODO. |
| **based-codegen** | ✅ **M2 (DDL) + M3 (read+write)** | `sql::ddl` → `CREATE TABLE`; `sql::dml` → query SELECTs; `sql::mutations` → INSERT/UPDATE/DELETE (soft-delete rewrite + scope injection). Client (M4) TODO. |
| runtime | ❌ not started | see Milestones. |

## based-sema — what it does now

Entry: `check(&[Decl]) -> (CheckedSchema, Vec<Diagnostic>)`.

Modules: `ir` (resolved types + codes + `Sink` + `snake_case`), `model` (AST model
→ `RModel`, two-phase), `resolve` (path resolution + the shared predicate/value
checker + `Cx` context), `check` (shapes/queries/mutations/filters + the four query
inferences), `lib` (orchestration).

Pass order (see `lib.rs`): collect+dedup → skeletons → validate (mut) → resolve
exprs (read-only) → check shapes/queries/mutations/filters. Split into mut/read
passes because scope/sort path resolution traverses *other* models while validate
holds `&mut`.

**Implemented checks**

- Name resolution: relation targets, inverse pairings (explicit `(M.field)` and
  inferred from the unique forward edge), shape `from`, return types, statement
  models, mutation write models, dotted paths (forward + backward traversal),
  index columns, `$param` refs (`$ctx` always allowed, D4), filter calls + arity,
  functions (closed set `KNOWN_FUNCS`).
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

**Diagnostic codes** live in `ir::code` (E01xx errors, W01xx lints). Parser owns
E0001/E0002, manifest E001x. Codes are stable — grep `ir.rs` for the registry.

**`CheckedSchema`** (the codegen seed): `models: Vec<RModel>` (fully resolved:
table name, members with kind Scalar/Forward/Inverse, soft_delete mode, sort,
scope, tenant, created/updated, indexes, unique_cols), plus resolved summaries
`shapes/queries/mutations/filters` and a `model_index` map. Codegen reads this
alongside the AST (`RQuery` carries inferred verb/target/many/paginated that are
*not* in the AST).

Tests: `crates/based-sema/tests/check.rs` (29 cases, positive + negative, keyed on
diagnostic codes). Commerce example (`spec/examples/commerce`) checks clean.

## based-sema — deferred (resume points)

Ordered by value. Each is a real gap with a known approach.

1. **Operand type-checking.** `resolve::Terminal` already carries the resolved
   type/target but callers ignore the payload (it's `#[allow(dead_code)]`). Add:
   op/value compatibility (`~` needs text, `in`/`has` need collection/json,
   comparisons need scalars), param explicit-type vs. mapped-column-type agreement
   (D1: a relation param may be typed `Id` or the target model), literal-vs-column
   type. Thread `Terminal` out of `check_value`/`resolve_path`.
2. **Named-filter body resolution.** A `filter` has no model at declaration, so its
   column paths are currently left unresolved (`check_predicate(.., None, ..)`).
   Resolve the body against each *call-site* model instead, and propagate
   soft-delete injection through filter calls. Note the `filter in_city(c) = … = c`
   spec example references params bare (no `$`) — decide: require `$c`, or treat a
   single-segment path matching a filter param as a param ref.
3. **Index lints (indexing.md).** Missing-index (`unindexed`) and useless-index are
   intentionally *not* implemented — they need the inferred baseline index set
   (join keys, filter paths, soft-delete columns) and "consequential table"
   heuristics (`unindexed(max_rows)`, `unindexed(unsafe)`), else they spam the
   reference schema with false positives. Build the inferred-index model first.
4. **`$ctx` typing (D4/D5).** Manifest must declare the `$ctx` shape; sema then
   types `$ctx.org` paths. Today `$ctx.*` is accepted unchecked.
5. **Relation `on:` custom joins.** The predicate uses table-qualified names
   (`orders.user_ref = users.legacy_id`) which the single-model path resolver
   can't handle; currently accepted unchecked. Needs a two-table resolution scope.
6. **`^` tx back-references (mutations.md).** Not in the lexer/AST yet — parser +
   AST work precede any sema check. `tx { create A{…}; create B{ x = ^.id } }`.
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
  - *Deferred inside M2*: the **inferred baseline index set** (join keys, filter paths,
    soft-delete columns → predicate-leading indexes, indexing.md). Needs the inference
    model (sema resume #3); emitting it blindly spams duplicate keys, and MariaDB has no
    partial indexes so "predicate-leading" = prepend `deleted_at`, a codegen concern for
    that pass. Today only *declared* structure is emitted.
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
  - *Deferred inside M3 read*: nested shape sub-objects (`field { … }` — needs JSON
    aggregation / a second query; skipped in projection); named-filter calls in `where`
    (filter bodies unresolved, sema #2 — rendered as a visible `TRUE /* … deferred */`
    no-op); `@tenant` injection (semantics unspecified vs. `@scope`); keyset cursor
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
  - *Deferred inside M3 write*: `^` tx back-references (`user = ^.id`) — not in the
    lexer/AST (sema resume #6), so a `tx` is a flat independent statement sequence;
    returning the declared shape after a write (RETURNING vs. re-select) — a runtime
    concern, no trailing SELECT emitted; required-field enforcement on `create`
    (sema resume #7) — an INSERT omits unassigned non-optional columns rather than
    erroring; raw write statements have no attached model so `{table}`/`{id}`
    interpolation has no root to bind.

**M4 — client codegen (`based gen client`).** One typed method + one wire route per
query/mutation (calling.md); input type from params, return from `-> Output`;
pagination envelope `{ rows, cursor }`. Rust target first (manifest `client`).

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
