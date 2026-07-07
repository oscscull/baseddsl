# PLAN.md — build-out roadmap

Working notes for whoever picks this up next. Records what's **done** (one line + the
governing D#), what's **open** (with enough context to resume without re-deriving), and
the **remaining milestones**. Spec is truth for *what* the language is; this is truth for
*where the implementation stands*.

> **Detail lives elsewhere on purpose.** The completed-milestone narration (what each
> shipped, why it was built that way) is in **`PLAN-archive.md`**; the per-decision record
> is in **`spec/decisions.md`** (D1–D50, with a topic index at its head). This file stays
> lean so resuming work doesn't cost a full history read. When a line below cites a `D#`,
> that decision entry (and the archive) is where the detail is.

## Autonomous build loop (how this is being built out)

This roadmap is executed by a self-driving loop. Protocol, for whoever (human or agent)
resumes it:

- **Optimize for the project being DONE, not for the loop continuing.** The measure is the
  Definition of Done below, not "a slice we can gate with `cargo test` today." Items are picked
  by *distance-closed to done*, hardest-critical-path-first. **Nothing is "blocked" merely because
  it needs Docker, a live DB, `brew`, or a non-Rust toolchain (TypeScript/npm)** — those are setup,
  not walls (OrbStack is installed; `brew` works; SQLite already runs in-memory). Do the setup.
  A deferred/nice-to-have item is worked ONLY when it is on the critical path or adds standalone
  value a user would notice; otherwise it stays deferred and out of the way.
- **One item per iteration, in fresh context.** Each iteration spawns ONE fresh
  general-purpose subagent that reads CLAUDE.md + `spec/principles.md` + this file +
  `spec/decisions.md`, picks the **highest-leverage item on the critical path to done** (see the
  Completion roadmap), implements it fully, and commits it. A fresh subagent per item is what keeps
  context clean between iterations (the whole point); the coordinator retains only one-line
  summaries, never the work.
- **Gate before commit.** `cargo test --workspace --all-features`, `cargo fmt --check`, and
  `cargo clippy --workspace --all-features` must all be clean. Never commit red. **Real-DB slices
  additionally gate on their live integration tests** — bring the DB up first (Docker, via the
  installed OrbStack: `docker run` an ephemeral Postgres/MariaDB, or testcontainers). A driver/live
  slice is not "done" until its real-DB test suite is green against a live server, not compile-verified.
- **Commit style.** On the current working branch (no push, no PR): first line
  `m6: <desc> (D<n>)`, short body, ending with the trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Update this file
  and `decisions.md` in the *same* commit as the code.
- **Sequential only.** Each iteration commits before the next starts (so the next reads
  updated state). The coordinator NEVER touches the repo/git while a subagent is running —
  they share one working tree + index.
- **Pause** after 3 items in a batch, or when a subagent hits a genuine blocker (it stops
  WITHOUT committing and reports), or when the unstarted items are exhausted.

**Where things stand (as of D50):** the architecture milestones (M2–M6) are done, and all three
target dialects (SQLite/MariaDB/Postgres) clear the real-DB bar. Remaining work is turning
"architecture-ready" into "a developer can adopt this" — see the Completion roadmap. Batch-by-batch
history is in `PLAN-archive.md`.

## Definition of Done (the product is complete when…)

Acceptance criteria. Everything in the Completion roadmap serves one of them. Status is the
current-truth summary; the evidence (which D# proved it) is in the archive.

1. **Proven against every target DB.** Each dialect the codegen emits has a concrete `Db`/`Backend`
   driver **and** a live integration suite running the *verbatim* `based gen sql` output against a
   **real server (Docker)** — not compile-verified, not MockDb. Per-DB coverage: get/list,
   `$ctx`-scope filtering (row + joined-`ON`), write + declared-shape re-select under one tx
   (read-your-writes), pagination, soft-delete/restore, idempotency dedupe, `Backend::ping`.
   **✅ Core met** — SQLite (D27), MariaDB Docker (D35), Postgres Docker (D38). *Remaining:*
   pagination + soft-delete/restore under the live suites (the A4 extras).
2. **A real, copyable example project per target DB.** A standalone Rust project (in-repo, **outside**
   the workspace, under `examples/`) consuming the generated client + runtime against a live DB — the
   thing a user copies to start. Builds in CI, doubles as an end-to-end smoke test. **🔴 Not started
   (Track B).**
3. **A functional, installable VS Code extension.** Packaged (`.vsix`), registers `.bsl`, launches
   `based-lsp`, surfaces diagnostics + inlay hints + hover + go-to-def + symbols + completion.
   **✅ Installable (D36); feature-parity fill-in in progress (Track C4).**
4. **Deployable + kept-proven.** A container image / Dockerfile for `based serve`, and CI running the
   real-DB suites + example builds + extension build so none of it rots. **🔴 Not started (Track D);
   the serve-side behaviour (health/readiness/drain) is done (D26).**
5. **Schema evolution: migration generation.** A `.bsl` change produces a reviewable, editable
   migration you can safely apply to an existing DB — not just from-scratch DDL. **✅ Core met** —
   spec (E1), snapshot + diff (D39), per-dialect render (D41), apply + `_based_migrations` ledger +
   status/verify + raw-SQL `down.mig` (D42), proven live. *Remaining:* E5 (`@was` renames + offline
   LSP drift diagnostic) + the `raw(dialect)` up step.

Deferred items (durable multi-instance idempotency store, shutdown grace deadline, incremental LSP
sync, rename, `^^` multi-level back-refs, self-ref join aliasing, nested shape sub-objects) stay
deferred — worked only if they land on the critical path or a user would notice their absence.

## Completion roadmap (ordered for velocity)

Five tracks. **A, C, and E are independent** (Rust drivers vs. the TS extension vs. the migration
engine — no shared files) so a coordinator may run them as parallel batches. B depends on A. D closes
it out (and its CI must cover E). Order *within* a track is top-down. Done items are one-liners with
their D#; open items carry full resume context. Delivery detail: `PLAN-archive.md`.

**Track A — real-DB proof (critical path, DoD #1).** *Mechanism: Docker (OrbStack).*
  - A1. ✅ **done (D35).** Docker-backed ephemeral-MariaDB test harness (`tests/support/docker_mariadb.rs`,
    feature `docker-tests`, skips cleanly with no daemon).
  - A2. ✅ **done (D35).** MariaDB live suite — verbatim codegen-lowered SQL through `serve::dispatch`
    against real `mariadb:11.4`, ran green.
  - A3. ✅ **done (D38).** Postgres driver + live suite (`src/postgres.rs`, `tests/postgres_integration.rs`),
    ran green against real `postgres:16`. All three dialects now clear DoD #1's real-server bar.
  - A4. 🔴 **OPEN. Live-DB hardening** — typed JSON reconstruction, statement timeouts, deadlock-retry,
    pool-exhaustion → 503 under load; verified against the live servers, not just designed. Also the
    remaining DoD-#1 coverage (pagination + soft-delete/restore) under the live suites.

**Track B — example projects (DoD #2, follows A per DB). 🔴 OPEN.**
  - B1. Scaffold `examples/` (standalone crates, non-workspace).
  - B2. One worked project per DB (SQLite first — its driver's already live — then MariaDB, then
    Postgres) consuming the generated client against the runtime; each builds + runs an end-to-end
    scenario in CI.

**Track C — VS Code extension (DoD #3, independent, may run in parallel).**
  - C1/C2. ✅ **done (D36).** Scaffolded `editors/vscode/` (TS + `vscode-languageclient`): `.bsl`
    registration, TextMate grammar, launches `based-lsp` over stdio, wires diagnostics/inlay/hover;
    `.vsix` packages.
  - C3. ✅ **done (D40).** Per-file manifest resolution — each open file resolves to its nearest
    `based.toml`, one snapshot per project, so embedded schemas resolve cross-file (no spurious E0110).
  - **C4. 🔴 NEXT PRIORITY. Feature-parity audit + fill-in** (baseline editor features a `.bsl` author
    expects). *Framing (user, 2026-07-06): the LSP exists to power the editor tooling, not the reverse.*
    The audit checklist lives in `editors/vscode/README.md` ("LSP capability audit"). Done so far:
    document symbols (D44), completion (D45), go-to-def (D43), find-references (D52). **Remaining:**
    workspace symbols (`⌘T`), rename (the `references_at` reference-site collector from D52 is the index
    it needs), folding ranges, selection ranges; code actions wiring lints to quick-fixes only if
    cheap. Also verify `language-configuration.json` covers bracket/auto-close/comment (`#`) — likely
    partial. Explicitly out of scope: formatting, signature help, call hierarchy, semantic-tokens re-do,
    debugging. **Acceptance:** each agreed gap implemented, capability-advertised, unit-tested against
    the commerce fixture, binary rebuilt. The **`based fmt` formatter** + `format-document` LSP
    directive are queued behind C4.
  - **C4a. ✅ done (D51) (user-raised 2026-07-07). Navigation + hover depth.** Three editor
    refinements the author noticed on the commerce `Order` model:
    - **Inverse inlay hint.** Was `inverse <- OrderItem via order` (wordy; `OrderItem[]` already on
      the line). Trim to `via order`, pushed to end-of-line, with the hint command-clickable to the
      forward edge it pairs with (`OrderItem.order`). Full "why" stays on hover. *(Fact gains a
      `nav` span → the paired forward member; LSP renders it as a clickable `InlayHintLabelPart`.)*
    - **Field-reference go-to-def.** Extend go-to-def past model/scope references to *field* paths:
      every `Path` segment in a shape body (`placed_by`, `placed_by.name`, `org.name`), a query
      `where`/`order`, and a mutation write's `where`/assign resolves to the field it names — each
      segment walked through relations from the statically-known root (shape `from`, query/statement
      target, write model). Filters (polymorphic call-site root) stay out. This is the reference-site
      resolver find-refs/rename will generalize.
    - **Broad hover ("what", rust-analyzer baseline).** Hovering any resolvable symbol shows its
      declaration: a field → `name: Type` (+ relation note), a model/shape/scope/callable reference or
      its own decl name → a one-line signature. Appended above the existing derived-fact "why".

**Track E — migration generation (DoD #5, independent, spec-first).** *Design settled 2026-07-06;
recorded in `spec/syntax/migrations.md` + D37. Model: declarative `.bsl` source, versioned artifacts
(`migrations/NNNN_slug/{up.mig,schema.snap}`), dialect-neutral step list rendered per-dialect over the
`Dialect` seam, offline/deterministic diff against the last stored snapshot, destructive changes loud
+ `--allow-destructive`-gated, renames never auto-guessed (explicit `@was`), roll-forward default with
optional hand-written `down.mig`, `_based_migrations` ledger with a tamper-hash.*
  - E1. ✅ **done.** `spec/syntax/migrations.md` — the spec, written first.
  - E2. ✅ **done (D39).** Snapshot + diff engine (`based-codegen::migrate` + `based migrate gen`):
    `CheckedSchema` → canonical neutral `schema.snap`; diff → the neutral `up.mig` step list;
    destructive steps marked. Offline, no DB.
  - E3. ✅ **done (D41).** Per-dialect renderer (`migrate::render_sql` + `based migrate render`): neutral
    steps → executable per-dialect SQL, reusing the DDL type map (can't drift from `based gen sql`);
    `alter column` diverges per dialect. Proven executable against real sqlite3/postgres:16/mariadb:11.4.
  - E4. ✅ **done (D42).** Apply + ledger (`based-runtime::migrate` + `based migrate apply|status|verify`):
    snapshot-authoritative execution, one tx per migration + ledger insert, FNV content-hash tamper
    guard, `--allow-destructive` gate, raw-SQL `down.mig` rollback, offline `verify` CI gate. Ran green
    against real mariadb:11.4 + SQLite.
  - E5. 🔴 **OPEN. `@was` rename directive** (sema) + the **offline schema-vs-migrations LSP drift
    diagnostic** ("N uncaptured changes — run `based migrate gen`"). Also the `raw(dialect)` up step.

**Track G — named + multi-scope (user-raised 2026-07-07). ✅ COMPLETE.** Scope is a first-class
**named** declaration referenced on both sides (`scope Name (col: Type = $ctx.field)`, `@scope Name`
on the model, `scoped Name` on the callable), because a contract this important must be *written, not
implied* (principle 2 — the old `@scope(pred)` inferred the `$ctx` type per callable and only *showed*
it). `@scope` is repeatable — commas within one decorator are AND, stacked decorators are OR (a DNF);
a callable confines by a set ⊇ one alternative. Landed across three iterations: named single-scope
(D48), multi-scope DNF with per-callable alternative injection + E0186 (D49), editor surface +
`schema.snap` serialization + UI decision-ref scrub (D50). Spec: D46/D47 + auth.md Handle 2. **Scope
rename is deferred to the C4 rename iteration** (needs the full reference-site index). Full iteration
detail: `PLAN-archive.md`.

**Track D — deploy + keep-proven (DoD #4, last). 🔴 OPEN.**
  - D1. Dockerfile / image for `based serve` (health/readiness + graceful drain already done, D26 —
    this is packaging).
  - D2. CI running the real-DB suites (A) + example builds (B) + extension build (C) + migration apply
    tests (E4) so the whole thing stays green.

**Track F — source hygiene pass (quality, cross-cutting; standalone value, off the DoD critical
path — worked when it won't preempt A/B/D/E).**
  - F1. **Finalize comments across all source.** Sweep every `crates/**/*.rs` and rewrite build-time /
    WIP narration into clean, **brief** what+why comments matching surrounding density. `sqlite.rs` is
    the known offender — do it first, then the rest. Source must read as finished source, not a scratch
    pad (narration reads as unfinished and leads humans *and* agents off task). Move inline TODOs into
    PLAN.md / the relevant roadmap `.md` unless genuinely must-do/blocking. Comment-only, so it gates on
    `cargo fmt --check` + `cargo clippy`. The standing rule is in Conventions below.

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
      ──codegen::migrate────▶ schema.snap + up.mig + per-dialect migration SQL (E2/E3 ✅ D39/D41)
      ──facts───────────────▶ engine-derived facts    (M5 ✅)
                              └─ based-lsp ──▶ editor inlay hints + hover + diagnostics + go-to-def + symbols + completion
      ──runtime::plan/run───▶ bound positional statement + shaped JSON  (M6 read+write ✅)
      ──runtime::serve──────▶ WireResponse (dispatch core; PlanError→4xx, DbError→503)  (M6 ✅)
      ──runtime::http───────▶ `based serve`: tiny_http listener over dispatch  (M6 ✅ D21)
                              └─ /healthz + /readyz probes + graceful drain (M6 ✅ D26)
      ──runtime::embed──────▶ in-process Engine (socket-free dispatch; typed client seam)  (M6 ✅ D22)
      ──runtime::{sqlite,driver,postgres}─▶ concrete Db/Backend per dialect + live integration tests  (M6 ✅ D27/D35/D38)
      ──runtime::migrate────▶ `based migrate apply`: live apply + _based_migrations ledger  (E4 ✅ D42)
```

`based check` wires discover → parse → sema → render. `based gen sql|client|openapi` and
`based migrate gen|render|apply|status|verify` all run the same front end (`load_checked` in
based-cli), then lower the `CheckedSchema`. All bail unless every file parses *and* checks clean
(codegen assumes a clean schema).

## Crate status

Current capability per crate. History (which D# added what) is in `PLAN-archive.md` + `decisions.md`.

| crate | state | what it does now |
|-------|-------|------------------|
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). `$ctx` is inferred in sema, not declared here (D4). |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. |
| based-sema | ✅ stable | resolution + checks + lints + `CheckedSchema` IR. Detailed behaviour in the next section. |
| based-cli | ✅ works | `based check`; `based gen sql\|client\|openapi`; `based facts [--json]`; `based migrate gen\|render\|apply\|status\|verify`; `based serve`. |
| based-codegen | ✅ stable | `sql::ddl\|dml\|mutations` → dialect-aware DDL/SELECT/INSERT-UPDATE-DELETE (MariaDB/SQLite/Postgres, D28/D29) through one `Dialect` quoting/type seam; `client` → typed Rust client; `openapi` → OpenAPI 3.1 (D24); `migrate` → `schema.snap`/`up.mig` diff (D39) + `render_sql` per-dialect migration SQL (D41) + `sql_statements`/`content_hash` for apply (D42) + scope serialization (D50). |
| based-facts | ✅ stable | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>` — the "show, don't write" facts (inferred inverses, join-key indexes, per-callable `$ctx` bags, resolved query shapes, scope contract), span-anchored, editor-string-scrubbed of internal refs (D50). |
| based-lsp | ✅ works (C4 in progress) | tower-lsp server; recompiles on edit (unsaved buffers overlaid on disk), publishes diagnostics + inlay + hover + go-to-def (D43) + document symbols (D44) + completion (D45); per-file manifest resolution (D40); scope go-to-def/hover (D50); field-reference go-to-def + broad declaration hover + command-clickable inverse inlay (D51); find-references incl. filter calls + inverse back-edge, filter go-to-def (D52). Remaining C4: workspace symbols, rename, folding. |
| based-runtime | ✅ works (M6) | in-process engine (D18): `Compiled::load` reuses the front end + codegen lowering; `plan_query`/`plan_mutation` validate + bind (`?`/`$n` per dialect), `run_*` shapes rows / runs writes under one tx with declared-shape re-select (D12). `serve::dispatch` is the wire core; `http` the `based serve` listener (D21) with health/readiness/drain (D26); `embed` the socket-free door (D22); `idempotency` keyed write dedupe + fingerprint (D25/D31). Concrete drivers: `sqlite` (D27), `driver::MariaDb` + `ShardRouter` (D20/D35), `postgres` + `PgRouter` (D38). `migrate` = live apply + ledger (D42). *Open:* live-DB hardening (Track A4); container image (Track D1); durable multi-instance idempotency store. |

## based-sema — what it does now

Entry: `check(&[Decl]) -> (CheckedSchema, Vec<Diagnostic>)`.

Modules: `ir` (resolved types + codes + `Sink` + `snake_case`), `model` (AST model
→ `RModel`, two-phase), `resolve` (path resolution + the shared predicate/value
checker + `Cx` context), `check` (shapes/queries/mutations/filters + the four query
inferences), `ctx` (`$ctx` per-callable inference + coherence, D4/D5), `scope` (named
scope resolution + DNF alternative injection, D48/D49), `indexes` (inferred-index model
+ the index lints, D15), `lib` (orchestration).

Pass order (see `lib.rs`): collect+dedup → skeletons → validate (mut) → resolve
exprs (read-only) → check shapes/queries/mutations/filters. Split into mut/read
passes because scope/sort path resolution traverses *other* models while validate
holds `&mut`.

**Implemented checks**

- Operand type-checking: op/operand applicability + operand family compatibility in `Cmp`
  (`E0150`/`E0151`); param annotation vs. mapped column (`E0152`, D1).
- Name resolution: relation targets, inverse pairings (explicit `(M.field)` and inferred from
  the unique forward edge), shape `from`, return types, statement models, mutation write models,
  dotted paths (forward + backward traversal), index columns, `$param` refs (`$ctx.<field>`
  structural check; type inferred per callable from use + coherence-checked, D4/D5), filter calls +
  arity *and* their bodies re-resolved against the call-site model (D14, cycle-guarded), functions
  (closed `KNOWN_FUNCS`), `^.field` tx back-references (D16: resolved against the immediately
  preceding `create`; `E0170` outside a tx / no prior create), custom `on:` join predicates (D17:
  two-table scope, table-qualified physical columns; `E0125`/`E0126`).
- `create` required-field enforcement (`E0146`): every non-optional, non-defaulted column / forward
  FK must be assigned; engine-managed fields (`id`, `@created`/`@updated`, `@soft_delete`) and
  custom-join forwards are exempt.
- `create`/`update` assign type agreement (`E0153`): assigned value's family must match the target
  column — the write-side twin of `=` operand typing; `^` back-refs typed by the field they read.
- Implicit `id: Id` (D2); a model that declares its own `id` keeps it.
- Decorators: `@soft_delete` (covered-subset type check → `SoftMode`), `@created`/`@updated`
  (timestamp role), `@sort` (paths), `@table` (name override), unknown `@foo` → `W0101`.
- **Named scope** (auth.md, D48/D49): a `scope Name (col: Type = $ctx.field)` decl (predicate = the
  restricted `col = $ctx.field` conjunction, checked at the decl site → `E0180`); `@scope Name`
  (repeatable → a DNF of alternatives) on the model; `scoped Name` / `unscoped("reason")` on the
  callable. Errors: `E0181` (create assigns a scope col), `E0182` (scoped callable declares neither
  scoped nor unscoped), `E0183` (unknown scope), `E0184` (model lacks the scope's column at a
  conforming type), `E0185` (scoped set ⊉ any alternative of a touched scoped model), `E0186`
  (a `create` can't auto-set a full alternative); `W0106` (stale unscoped). Scope injected into the
  root/write-target `WHERE` *and* every joined scoped model's `ON` (D34); shard key bound to the
  scope `$ctx` field (D33).
- Table naming (D3): `snake_case`, no pluralization, `@table("…")` override. Relation FK column =
  `<field>_id` or `(column "…")`.
- Query inferences (queries.md): target model (from return shape's `from`), verb (`get`/`list`), same-
  name param→column mapping, per-param bindings (`-> edge`, `op col`). `get` must be keyed on a unique
  field → `E0144`.
- Duplicates: model / shape (except `full`) / callable (query+mutation share the wire namespace) /
  filter / field.
- Lints: `W0100` nondeterministic `list`, `W0102` raw SQL on a `@soft_delete` model, and the index
  lints (indexing.md, D15, `indexes.rs`): `W0103` missing-index (satisfied by `@index` or
  `unindexed(…)`), `W0104` useless-index, `W0105` stale annotation. Traversed inverse edges seed
  `RModel.inferred_indexes` (join-key baseline; DDL emits them `inf_`-prefixed, soft-delete-leading).

**Diagnostic codes** live in `ir::code` (E01xx errors, W01xx lints). Parser owns E0001/E0002,
manifest E001x. Codes are stable — grep `ir.rs` for the registry.

**`CheckedSchema`** (the codegen seed): `models: Vec<RModel>` (fully resolved: table name, members
with kind Scalar/Forward/Inverse, soft_delete mode, sort, scope, created/updated, indexes,
unique_cols), resolved summaries `shapes/queries/mutations/filters`, a `model_index` map, and
`scopes` (the named scope decls). Codegen reads this alongside the AST (`RQuery` carries inferred
verb/target/many/paginated that are *not* in the AST).

Tests: `crates/based-sema/tests/check.rs` (~109 cases, positive + negative, keyed on diagnostic
codes) + `tests/conformance.rs` (a golden harness over `tests/conformance-sema/<case>/`, re-bless with
`BLESS=1`). Commerce (`spec/examples/commerce`) checks clean.

## Conventions

- Rust workspace, edition 2021, rust-version 1.85. `cargo test` / `cargo clippy` /
  `cargo fmt --check` must stay clean (stock rustfmt, no config).
- Diagnostics carry spans (`FileId` + byte range); `based-cli/src/render.rs` frames
  them rustc-style. New checks → new stable code in `ir::code` + a note when the fix
  isn't obvious from the message.
- Audience is LLMs + reviewers: optimize tokens-to-comprehend, readable > terse
  (CLAUDE.md). Match surrounding comment density.
- **Comments state what + why, briefly — never build-time narration.** Source is finished
  source, not a scratch pad: no "here's what I'm building" / WIP running commentary (it reads
  as unfinished and leads humans *and* agents off task). TODOs live in PLAN.md / roadmap `.md`,
  not inline, unless genuinely must-do/blocking. (One-time cleanup of existing narration = Track F1.)
- **Keep this file lean.** PLAN.md is the resume read; shipped-work narration goes to
  `PLAN-archive.md`, per-decision detail to `spec/decisions.md`. Add a one-line status + D# here,
  not a paragraph.
- `spec/principles.md` are the tiebreakers, in order. `spec/decisions.md` (with its topic index)
  resolves anything the prose left open.
