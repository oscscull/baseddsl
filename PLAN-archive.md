# PLAN-archive.md — delivery log (historical)

Frozen detail moved out of PLAN.md to keep the resume read lean. This is the
**completed-work narration**: what each milestone/track shipped, in the words written
at the time. Nothing here is required to resume work — PLAN.md carries the live status
and the open items, and `spec/decisions.md` carries the per-decision (D#) record.
Kept for archaeology: the "why did we build it this way" detail behind a shipped D#.

Snapshot as of **D50**. Superseded framing is preserved as-written; trust PLAN.md +
decisions.md for current truth.

---

## Batch history (autonomous build loop)

Batch progress: current batch — **D32 (`@scope` resolved: uniform single-owner row filter — a
conjunction of `col = $ctx.field`, `E0180`; create-time auto-set of the scope column from `$ctx`
so cross-scope create is inexpressible, `E0181`; the `unscoped("reason")` escape hatch, `W0106`;
resolves D19)** + **D33 (shard key bound to the resolved `@scope` `$ctx` field: `RModel::
shard_key_ctx_field` → per-callable `RQuery`/`RMutation.shard_key`, derived from the same `@scope`
that filters the row so routing and row-visibility can't drift; `unscoped` → no owning shard;
listener `http::resolve_shard_key` pulls the field out of `$ctx`, `X-Based-Shard-Key` override
retained; retires the hand-set `--shard-key-field` flag; closes the D20/D32 follow-on)** + **D34
(`@scope` injected into a *joined* table's `ON`: a query/mutation reaching a *different* scoped
model through a relation now filters that joined model by its `@scope` in the join `ON` — the same
slot soft-delete uses — closing the cross-scope leak D32 left open; `Select::scope_join_pred` binds
the shared `:ctx_<field>`, sema's `ctx` collector requires the joined field so the bind is present,
`unscoped` drops the joins too; proven end-to-end against live SQLite)** done. Completion batch —
**D35 (Track A1+A2: the Docker-backed ephemeral-MariaDB integration harness —
`tests/support/docker_mariadb.rs`, feature `docker-tests`, skips cleanly with no daemon — and a MariaDB
live suite running the verbatim codegen-lowered SQL against a real `mariadb:11.4` via the `MariaDb`
driver/`ShardRouter`; full DoD-#1 coverage, ran genuinely green — the compile-verified driver is now
proven)** + **D38 (Track A3: the concrete Postgres `PostgresDb`/`Backend` + bounded-pool `PgRouter` behind
feature `postgres`, running the *verbatim* Postgres-lowered `$n`-bound SQL against a real `postgres:16` over
the twin harness `tests/support/docker_postgres.rs`; the crux was the `SqlValue`↔Postgres value mapping — a
`PgValue` `ToSql` newtype that text-format-encodes strings so the server string-coerces them into
`uuid`/`timestamptz`/`jsonb`, no per-column types in the runtime; live suite `tests/postgres_integration.rs`,
7 tests, ran genuinely green — all three target dialects now clear DoD #1)** done.
Also this batch — **E2 (Track E: migration snapshot + diff engine — `based-codegen::migrate` + `based
migrate gen`; serializes `CheckedSchema` → the canonical dialect-neutral `schema.snap`, diffs a prior
snapshot vs. the current schema → the neutral `up.mig` step list, marks destructive steps; offline +
deterministic, no DB; D39)** + **E3 (Track E: per-dialect migration renderer — `based-codegen::migrate::
render_sql` + `based migrate render`; neutral `up.mig` steps → executable `CREATE`/`ALTER`/`DROP` SQL over
the `Dialect` seam, reusing the DDL type map so a migration can't drift from `based gen sql`; `alter column`
diverges per dialect — Postgres piecemeal, MariaDB `MODIFY COLUMN`, SQLite a loud raw-rebuild comment;
render re-derives steps from the stored snapshots so no `up.mig` parser yet; proven executable against real
sqlite3/postgres:16/mariadb:11.4 for create/add/alter/index; D41)** + **E4 (Track E: migration apply +
`_based_migrations` ledger — `based-runtime::migrate` over the `Db` seam + `based migrate apply|status|verify`;
snapshot-authoritative execution via the new `migrate::sql_statements` (so applied == reviewed SQL), one tx per
migration + ledger insert, FNV `content_hash` tamper guard, `--allow-destructive` gate, raw-SQL `down.mig`
rollback (`--down`/`--to N`), offline `verify` CI gate; **ran genuinely green against real mariadb:11.4 (Docker)
+ SQLite in the normal gate**; closes DoD #5's apply half; D42)** done.
Prior batch: D29 (Postgres dialect: `ddl`/`dml`/`mutations` codegen + the dialect-aware `?`→`$n`
scanner; the concrete driver deferred to the live-DB slice) + D30 (typed per-callable `$ctx` in
the generated Rust client) + D31 (idempotency-key request fingerprint: a reused key on different
args → loud `422`, not a silent replay of the first request) — 3/3.

**Reoriented 2026-07-06 toward completion (see below).** The architecture milestones (M2–M6) are
done; what remains is turning "architecture-ready" into "a developer can actually adopt this." The
prior framing parked the real remaining work as "blocked — needs infra"; it is not blocked. The
sections below define *done* and order the path to it.

---

## Completion roadmap — full historical detail (as of D50)

> Frozen copy. Open items (A4, B, C4, E5, D, F) also live in PLAN.md, which is
> authoritative for them; the entries below preserve the done-track delivery prose.

## Completion roadmap (ordered for velocity)

> ✅ **C3 done (D40)** — the LSP now resolves each open file to its owning `based.toml` by walking up its
> ancestors and compiles one snapshot per project, so embedded schemas (the "ride along inside a Rust repo"
> case) resolve cross-file references. See Track C3 below for the resume note.
>
> ✅ **DONE — Track G (named scope — a user-raised language change).** Scope is a first-class **named**
> declaration referenced on both sides (`scope Name (col: Type = $ctx.field)`, `@scope Name` on the model,
> `scoped Name` on the callable). The user's framing (2026-07-07): "a scope contract this important must be
> *written, not implied*" — the old `@scope(pred)` inferred the `$ctx` type per callable and only *showed* it
> as an editor hint (D4/D5), which principle 2 forbids for a consequential contract. **✅ Iteration 1 (named
> scope, single-scope) D48; ✅ iteration 2 (multi-scope DNF) D49; ✅ iteration 3 (editor surface + snapshot +
> UI scrub) D50.** The scope feature is fully landed. Scope **rename** across refs is deferred to the C4 rename
> iteration (it needs the full reference-site index C4 builds). See Track G below.
>
> **Track C4 (VS Code feature-parity fill-in)** is the next queued work — remaining: workspace symbols,
> find-refs, rename, folding (document symbols D44 + completion D45 done). The **`based fmt` formatter** +
> the `format-document` LSP directive are queued behind C4. (The D4/D5-hover scrub is done — D50.)

Five tracks. **A, C, and E are independent** (Rust drivers vs. TypeScript extension vs. the migration
engine — no shared files) so a coordinator may run them as parallel batches. B depends on A. D closes
it out (and its CI must cover E). Order *within* a track is top-down.

**Track A — real-DB proof (critical path, DoD #1).** *Mechanism decided: Docker (OrbStack, installed).*
  - A1. ✅ **done (D35). Docker-backed test harness** — `crates/based-runtime/tests/support/docker_mariadb.rs`:
    a thin `docker run` guard behind feature `docker-tests` that brings up an ephemeral pinned
    `mariadb:11.4` on a random free port, polls a real connection for readiness, and force-removes the
    container on `Drop` (a panicking test still cleans up). No daemon ⇒ `MariaDbContainer::start()`
    returns `None` and each test **skips cleanly** (logs a reason), so `cargo test --all-features`
    stays green with or without Docker. Chosen over testcontainers-rs to avoid pulling an async runtime
    into the sync codebase (principle 7 — reuse the `docker` CLI). Ready to host the Postgres suite (A3).
  - A2. ✅ **done (D35). MariaDB live suite** — `crates/based-runtime/tests/mariadb_integration.rs`
    (7 tests, the MariaDB twin of `sqlite_integration.rs`): loads the real commerce schema
    (`Compiled::load`, MariaDB dialect), creates tables from the *generated* MariaDB DDL
    (`sql::ddl(_, Dialect::MariaDb)`), and drives requests through `serve::dispatch` against the
    concrete `MariaDb` driver checked out of a live `ShardRouter`. Runs the **verbatim** codegen-lowered
    SQL (`?`-bound), no MockDb. Covers get/list, `$ctx`-row-scope filtering (+ the joined-`ON` reach
    projects live), a write + declared-shape re-select under one tx (read-your-writes), idempotency-key
    dedupe, and `Backend::ping`. **Ran genuinely green against real MariaDB 11.4** (not compile-verified);
    this is now the `MariaDb` driver's real gate. Note: MariaDB's native `UUID` column rejects non-UUID
    ids, so the suite pulls `serve` for the production `UuidGen` + uses valid UUID fixtures (D35).
    *Still deferred to A4 (live-DB hardening):* typed JSON reconstruction, statement timeouts,
    deadlock-retry, pool-exhaustion → 503 under load — designed (D20/D26), not yet stress-proven live.
  - A3. ✅ **done (D38). Postgres driver + live suite** — the concrete `postgres` `Db`/`Backend`
    (`crates/based-runtime/src/postgres.rs`, feature `postgres`): [`PostgresDb`] over one pooled
    connection (pure-Rust **sync** `postgres` crate, no async runtime — D20) + [`PgRouter`], the
    `ShardRouter` twin (one bounded `r2d2` pool per shard, same stable FNV logical-shard routing,
    now in the backend-agnostic `src/shard.rs`). TLS off (no system OpenSSL dep), mirroring MariaDB.
    Runs the **verbatim** Postgres-lowered SQL (`$n`-bound, D29). The crux was the value mapping: a
    dialect-neutral `SqlValue::Text` (uuid/timestamptz/jsonb all ride as strings, D1) is bound via a
    `PgValue` `ToSql` newtype that `accepts` those OIDs and encodes in **text format**, so the server
    string-coerces it (the `'…'::uuid` path) — no per-column Postgres types in the runtime; unit-tested
    like `from_mysql`. Live Docker suite `tests/postgres_integration.rs` (7 tests, the Postgres twin of
    the MariaDB suite) over the new harness `tests/support/docker_postgres.rs` (ephemeral `postgres:16`,
    skips cleanly with no daemon): loads commerce lowered for `Dialect::Postgres`, creates the generated
    Postgres DDL, and drives get/list, `$ctx` row-scope + joined-`ON` reach, write + declared-shape
    re-select under one tx (read-your-writes), idempotency dedupe, `Backend::ping`. **Ran genuinely
    green against real Postgres 16** — this is now the `PostgresDb` driver's real gate. Every dialect
    the codegen emits (SQLite/MariaDB/Postgres) now clears DoD #1's real-server bar.
  - A4. **Live-DB hardening** — typed JSON reconstruction, statement timeouts, deadlock-retry,
    pool-exhaustion → 503 under load; verified against the live servers, not just designed.

**Track B — example projects (DoD #2, follows A per DB).**
  - B1. Scaffold `examples/` (standalone crates, non-workspace). B2. One worked project per DB
    (SQLite first — the driver's already live — then MariaDB, then Postgres) consuming the generated
    client against the runtime; each builds + runs an end-to-end scenario in CI.

**Track C — VS Code extension (DoD #3, independent, may run in parallel now).**
  - C1. ✅ **done (D36).** Scaffolded `editors/vscode/` (TS + `package.json` + `vscode-languageclient`):
    `.bsl` language registration + minimal TextMate grammar/`language-configuration.json`, launches
    `based-lsp` over stdio (`basedls.serverPath`, defaults to PATH), wires diagnostics/inlay/hover.
    C2. ✅ **done (D36).** `npm run compile` (tsc) clean; `.vsix` packages via `npx @vscode/vsce package`;
    README covers building `based-lsp`, `npm install`/compile, and package/install. Gating is `tsc` +
    `vsce package` (no cargo twin).
  - C3. ✅ **done (D40). Per-file manifest resolution (embedded-schema support).** The LSP no longer roots
    at a single workspace folder. `compile::find_manifest_root(file)` walks the file's ancestors to the
    **nearest** `based.toml` (rust-analyzer/tsserver project-marker model; `crates/based-lsp/src/compile.rs`);
    a `ProjectKey` (`Manifest(root_dir)` | `Loose(file)`) names the project each open file belongs to.
    `State` dropped the single `root`/`snapshot` for `snapshots: HashMap<ProjectKey, Snapshot>` + a
    `published: Vec<Url>` (to clear a project's squiggles when it drops out of the open set). `refresh`
    groups open buffers by project and compiles **one snapshot per project** (`compile_manifest` = the D5
    glob; `compile_loose` = the single-file fallback for a file under no manifest), then publishes each file
    from its **owning** project only — a nested manifest's file also appears in an outer glob, so the nearest
    owns it (no double-publish). `inlay_hint`/`hover`/diagnostics route to `snapshots.get(&project_key(path))`.
    **Result: opening the repo root (no `based.toml` there) and editing `commerce/order/model.bsl` resolves
    `Org`/`User`/`OrderItem` across sibling files — no spurious `E0110`; two embedded schemas in one
    workspace resolve independently.** Recorded as **D40**. Tests: `find_manifest_root_walks_up_to_nearest_manifest`
    + `two_manifest_workspace_resolves_each_project_independently` (proves the manifest scope fixes the very
    `E0110` a `compile_loose` still shows), plus the existing commerce snapshot test moved to `compile_manifest`.
    *Deferred:* a not-yet-saved new file isn't in its manifest's on-disk glob, so it compiles loose until first
    save (same edge as before); no proactive whole-workspace discovery on `initialize` — projects compile
    lazily as their files open (fine for a per-file editor surface).
  - **C4. 🔴 NEXT PRIORITY. Feature-parity audit + fill-in (basic editor features a `.bsl` author expects).**
    *Framing (user, 2026-07-06): the LSP exists to power the editor tooling, not the reverse — features are
    driven by "what should a language extension do," and the server grows to serve them.* The extension was
    built bottom-up from what the engine already derived (diagnostics/inlay/hover), so **table-stakes IDE
    features were skipped** — go-to-definition was absent until a user hit it (just added as a Track C
    follow-up, D43). Before any exotic work, **audit the extension against what a "normal" language extension
    provides and fill the gaps.** Nothing fancy — the expected baseline only. Concretely:
    - **Produce the checklist first** (a short section here or in `editors/vscode/README.md`): for each
      standard LSP capability, mark have / missing / N/A-for-a-DSL, so the gap set is explicit and reviewable.
      ✅ **done (D44)** — the audit table lives in `editors/vscode/README.md` ("LSP capability audit"): have
      = diagnostics/inlay/hover/go-to-def/document-symbols/TextMate; missing = completion, workspace symbols,
      find-refs, rename, folding, selection ranges, code actions; deferred/N-A = formatting, signature help,
      call hierarchy, semantic-tokens re-do, debugging (with reasons). This governs the remaining C4 items.
    - **Known-present:** diagnostics ✅, inlay hints ✅, hover ✅, go-to-definition ✅ (D43),
      TextMate coloring (models vs. builtins) ✅ (D43). Verify `language-configuration.json` covers
      bracket matching / auto-closing pairs / comment toggling (`#`) — cheap, static, likely partial.
    - **Likely-missing baseline to fill** (implement the ones that are genuinely expected; each is one LSP
      request backed by the already-parsed `Snapshot.decls`, so cost is low):
      • **document symbols** (outline / breadcrumbs / `⇧⌘O`) — models, queries, mutations, shapes, filters as
        a flat/nested symbol tree; probably the highest value-per-effort. ✅ **done (D44)** —
        `Snapshot::document_symbols(fid)` over the retained `decls` (model→Struct + fields as Field children,
        shape→Interface, query→Function, mutation→Method, filter→Function), advertised at `initialize`,
        unit-tested against commerce. Next up: completion.
      • **completion** — model names in type position, field names after `.`, the keyword/decorator set; the
        one feature an author feels the absence of constantly. ✅ **done (D45)** — `Snapshot::completions(fid,
        offset)` classifies by the source *prefix* trigger char (no parse of the half-typed buffer): `@` →
        decorators (`KNOWN_DECORATORS` + `index`/`was`), `<ident>.` → the base model's fields (path rooted at a
        shape's `from` / query block's target, walked through relations; else nothing — precision over recall),
        `:` → primitives + models, `->` → models + shapes, else the keyword/function vocabulary + models.
        Advertised via `completion_provider` (trigger chars `.` `@`); unit-tested over a hermetic manifest.
        Next up: workspace symbols.
      • **workspace symbols** (`⌘T`) — jump to any model/callable by name across the project.
      • **find-references + rename** — the reference-site index is a superset of the go-to-def collector
        (already noted as the go-to-def resume point); rename is the natural pair.
      • **folding ranges** (block folding), **selection ranges** — minor, cheap, expected.
      • **code actions** wiring the existing lints to quick-fixes (e.g. `W0103` → "add `@index`") — borderline;
        include only if it falls out cheaply, else defer.
    - **Explicitly out of scope (exotic / not baseline):** formatting (no `based fmt` yet), signature help,
      call hierarchy, semantic-tokens re-do of coloring, debugging. List them as deferred, don't build them.
    - **Acceptance:** the checklist exists; the agreed baseline gaps are implemented, capability-advertised,
      unit-tested against the commerce fixture (same style as the go-to-def test), and the binary rebuilt so the
      live extension surfaces them. Split into per-feature commits if large. *(Raised by the user 2026-07-06.)*

**Track E — migration generation (DoD #5, independent, spec-first).** *Design settled 2026-07-06 with
the user; see the decision block below. `spec/syntax/migrations.md` is written before any code.*

  Settled model — **declarative source, versioned artifacts.** The `.bsl` schema stays the single
  source of truth (P4); migrations are the generated, reviewable, editable derivative that carries a
  DB from schema-state N→N+1 (the Prisma/Atlas *versioned* model, NOT live declarative-apply). Settled
  decisions (the forks the user resolved):
  - **Directory of versioned migrations**, kept updated by `based migrate gen` (diff schema vs. last
    captured state). `migrations/NNNN_slug/` per migration.
  - **Baseline = stored schema snapshot per migration** (`schema.snap`) — diff = current `.bsl` vs.
    the latest snapshot. Fully **offline/deterministic**, git-diffable, no DB to generate. *(user)*
  - **Canonical artifact = dialect-neutral step list** (`up.mig`, in the schema's own IR vocabulary),
    rendered to per-dialect SQL (SQLite/MariaDB/Postgres) at apply time via the existing `Dialect`
    seam (P4 — can't drift), **plus a first-class `raw(dialect) \`…\`` escape step** for data migrations /
    anything the neutral vocabulary can't express (mirrors `raw.md`). *Rationale (resolves the user's
    neutral-vs-raw torn-ness):* the neutral format is what makes the two choices above actually
    compose — the snapshot baseline and the offline editor drift check (Track E5) both need the tool to
    answer "what schema do these migrations produce?" **without a DB**, which is only tractable if the
    steps are machine-understandable. Raw SQL would be opaque offline (needs a SQL parser or a shadow
    DB, which the user declined for the baseline). So: neutral for structural DDL (keeps snapshots
    honest + drift check working infra-free); raw escape where SQL is genuinely the right tool, with
    that migration visibly marked "not offline-verifiable for the raw step."
  - **Rollback = roll-forward by default; an OPTIONAL author-supplied `down.mig` is honored if
    present, never auto-generated** (no fake reverses). *(user)*
  - **Destructive changes loud + guarded** (P1): drops / type-narrowing / new `NOT NULL` without a
    default / new unique over existing data are generated but require an explicit `--allow-destructive`
    / `unsafe("reason")` ack to apply — never silent data loss.
  - **Renames never auto-guessed**: default emits drop+add (safe, visible); an explicit **`@was("old")`**
    directive in `.bsl` declares a rename → a clean `RENAME` step. This is the user's "adjustable to
    match an old coherent schema" requirement — the generated migration is a proposal you correct.
  - **Applied-state ledger**: a `_based_migrations` table (id + content-hash + timestamp); a migration
    whose hash changed after it was applied → loud error, never a silent re-apply.
  - **Editor/LSP drift = offline schema-vs-migrations only** *(user)*: a diagnostic when the `.bsl`
    schema has changes not yet captured in a migration ("N uncaptured changes — run `based migrate
    gen`"). Reuses the `based-facts`/diagnostics infra, no DB. Live-DB drift stays a CLI concern.
  - `based gen sql` stays as the from-scratch full snapshot; `0001_init`'s up == that.

  Track E items (top-down):
  - E1. ✅ **done. `spec/syntax/migrations.md`** — the spec, written FIRST: the declarative-source /
    versioned-artifacts model, the `migrations/NNNN_slug/` layout, the dialect-neutral `up.mig` step
    vocabulary (add/drop/alter table/column/index/unique, rename-via-`@was`) rendered per-dialect over
    the `Dialect` seam with a worked commerce example (nullable add + index, all three dialects), the
    first-class `raw(dialect)` escape (marked "not offline-verifiable"), the `schema.snap` canonical
    stable-ordered neutral serialization, the `@was("old")` rename directive, the destructive-change
    policy (`--allow-destructive` / `unsafe("reason")`), roll-forward default + optional `down.mig`,
    the `_based_migrations` ledger + tamper/hash rule, the offline LSP drift diagnostic, and the
    `based migrate gen|apply|status|verify|render` surface. Extended `spec/grammar.ebnf` (`@was` as a
    field `modifier`/model decorator) + the CLAUDE.md spec file map. Open sub-details flagged inline
    as TODOs for E2–E5 (snapshot grammar pin, raw-step structural-effect annotation, hash algo/canon,
    down-invocation surface). **E4 (apply + ledger) is next.**
  - E2. ✅ **done (D39). Snapshot + diff engine** — `based-codegen::migrate` (over the same `Dialect`
    seam E3 renders on): [`snapshot`] serializes `CheckedSchema` → the canonical stable-ordered
    dialect-neutral `schema.snap` (tables/columns/indexes sorted by name, `id` elided as the D2
    invariant, soft-delete/created/updated roles + `@scope`/`@sort` in the header — pure, no wall-clock);
    a `Snapshot` round-trips (`render`/`parse`) so the stored baseline is diffable; `diff(prev_snapshot,
    schema)` → the neutral `up.mig` [`Step`] list (create/drop table, add/drop/alter column, add/drop
    index/unique). `0001_init` diffs against the empty schema → a full create set (== `based gen sql`
    from scratch); renames are drop+add (never auto-guessed — `@was` is E5); destructive steps (drops,
    narrowing, new not_null w/o default, new unique) are *marked* `Step::destructive()` for E4's gate
    (marked, never applied). `based migrate gen [name]` (based-cli) loads the checked schema, finds the
    latest `migrations/NNNN_*/schema.snap`, diffs, and writes the next zero-padded `NNNN_slug/{up.mig,
    schema.snap}` (NNNN from counting dirs, not time); no changes ⇒ writes nothing. Golden `schema.snap`
    for commerce (re-blessable) + diff/destructive unit tests + a temp-dir CLI test. Finalized
    migrations.md's snapshot-grammar TODO (the `snapshot v1`/`table`/`column`/`index` block).
  - E3. ✅ **done (D41). Per-dialect renderer** — `based-codegen::migrate::render_sql(&[Step], Dialect)`:
    the neutral `up.mig` steps → executable `CREATE`/`ALTER`/`DROP` SQL over the existing `Dialect` seam.
    Reuses the DDL type map (`sql::sql_type`, now `pub(crate)`) and `Dialect::quote`/`bool_lit`, so a
    migration's SQL can't drift from `based gen sql` (P4) — `0001_init`'s create steps render to the same
    DDL from scratch. `CreateTable` re-synthesizes the elided implicit `id` PK (D2); `(unique)` columns →
    `CONSTRAINT … UNIQUE`; indexes inline as `KEY`/`UNIQUE KEY` on MariaDB, trailing `CREATE INDEX`
    elsewhere. **`alter column` is the dialect-divergent case:** Postgres emits one `ALTER COLUMN …` per
    change; MariaDB restates the whole column via `MODIFY COLUMN` (no piecemeal form — so `Step::AlterColumn`
    grew an `after: ColumnSnap` carrying the resulting column state); SQLite has *no* in-place `ALTER COLUMN`,
    so it renders a loud comment pointing at a hand-authored `raw(sqlite)` table-rebuild (principle 6 — never
    silent broken SQL). `DROP INDEX` also branches (MariaDB needs `ON <table>`). Destructive steps get a loud
    `-- DESTRUCTIVE` marker. `based migrate render [--number NNNN] [--dialect D]` (based-cli): re-derives each
    migration's steps as `diff_snapshots(snapshot[N-1], snapshot[N])` from the stored `schema.snap`s
    (snapshot-authoritative — what `verify` asserts equals the `up.mig`), so **no `up.mig` text parser is
    needed here** (that lands with E4/apply, which must parse + hash `up.mig`); `--dialect` overrides the
    manifest for a cross-target review, default is the manifest dialect. Tests: 7 render unit tests in
    `migrate.rs` (create/add/drop/alter/index across all three dialects, destructive markers, MariaDB
    default-only-alter-avoids-MODIFY) + a commerce init render integration test (one `CREATE TABLE` + PK per
    model per dialect, type-map cross-check vs. `sql::ddl`). **Proven executable against real servers:** the
    commerce `0001_init` + an incremental `0002` (add column + nullable-alter + index) render was applied end
    to end against real `sqlite3`, `postgres:16`, and `mariadb:11.4` (Docker) — every dialect's create,
    add-column, alter-column, and index SQL runs. *Deferred:* the `raw(dialect)` passthrough step (the
    `Step` enum has no raw variant yet — migrations.md's raw-structural-effect TODO); rename steps (E5,
    `@was`); honoring hand-edited `up.mig` in render (needs the E4 parser).
  - E4. ✅ **done (D42). Apply + ledger** — `based-runtime::migrate` (dialect-generic over the `Db` seam) +
    `based-codegen::migrate::{sql_statements, content_hash}` + CLI `based migrate apply|status|verify`.
    `load_migrations` reads `migrations/NNNN_*/`, re-derives each migration's steps from the stored
    `schema.snap`s (snapshot-authoritative, same as `render` — **there is no `up.mig` parser**; the neutral
    text isn't losslessly parseable, so the snapshot chain is truth) and lowers them via the new
    `sql_statements` (the execution twin of `render_sql` — bare statements through one `step_statements` seam,
    so applied SQL == reviewed SQL, P4). `apply` runs each migration's statements **+ its `_based_migrations`
    ledger insert under one tx** (best-effort on MySQL's implicit-commit DDL; a re-apply skips completed
    migrations by id), gates destructive steps on `--allow-destructive`, and enforces the tamper guard
    (`content_hash` = FNV-1a-64 over canonicalized `up.mig`; an edited applied migration → hard `Tamper` error)
    + the contiguous-prefix ledger invariant. Rollback: an OPTIONAL **raw-SQL** `down.mig` (D42 — neutral-down
    is inexpressible without a lossless parser; a hand-written reverse is naturally SQL) honored by `--down`
    (latest) / `--to NNNN` (reconcile to `{≤N}`), each deleting its ledger row in-tx; no `down.mig` → loud
    `NoDown`. `based migrate status` shows applied/pending + hash mismatches; `based migrate verify` is the
    offline CI gate (re-render each up.mig from its snapshots + compare hash; latest snapshot == current `.bsl`,
    i.e. no uncaptured drift). CLI now links all three drivers and picks by manifest dialect; `--database-url`
    repeatable → migrate every shard (D20). **Tests ran genuinely green:** `tests/migrate_apply.rs` (SQLite
    in-memory, in the normal gate — fresh apply+ledger, re-apply no-op, status, `down.mig` rollback, tamper,
    destructive gate) + `tests/migrate_apply_mariadb.rs` (the D35 Docker harness — apply against real
    `mariadb:11.4`, ledger + column verified, tamper, re-apply no-op) + codegen/runtime unit tests. **DoD #5's
    apply half is met.** *Deferred:* multi-instance apply coordination (racing deployers — parallels D25);
    `@was` renames + the LSP drift diagnostic (E5); the `raw(dialect)` up step.
  - E5. **`@was` rename directive** (sema) + the **offline schema-vs-migrations LSP drift diagnostic**.

**Track G — named + multi-scope (DoD-adjacent language change; user-raised 2026-07-07).**
> ✅ **Iteration 1 (named scope) landed as D48.** Inline `@scope(pred)` is replaced by the named surface:
> `scope Name (col: Type = $ctx.field)` decl, `@scope Name` on the model (→ `Model.scopes: Vec<ScopeRef>`,
> a DNF-ready list of alternatives), `scoped Name` on the callable (`scope_ack`, mutually exclusive with
> `unscoped`). Sema builds `CheckedSchema.scopes`/`RScope`, moves `E0180` to the decl site, adds
> `E0182`/`E0183`/`E0184`/`E0185` (+ the touched-scoped-model superset rule over `RModel.scope_alts`), and
> **synthesizes `RModel.scope: Option<Predicate>` from the chosen alternative** so codegen/runtime are
> untouched (golden SQL unchanged; sema conformance golden byte-identical). Scope-field `$ctx` type sourced
> from the decl (ends D4/D5 for it; coherence structural). Commerce migrated (`scope Tenant`, `@scope
> Tenant` on `Order`, `scoped Tenant` on its callables) and checks clean.
>
> ✅ **Iteration 2 (multi-scope DNF) landed as D49.** Codegen now injects the *callable-chosen* alternative,
> not the single synthesized `RModel.scope`: sema resolves `RQuery`/`RMutation.scope_inject: Vec<ScopeInject>`
> (per touched scoped model, the chosen axes' `(column, ctx_field)` terms — `scope::resolve_inject`), threaded
> into `Select::scope_where`, which replaces every read of `RModel.scope`/`scope_terms()` in the SQL path
> (root `WHERE`, joined `ON`, create auto-set, write guards, restore, D12 re-select). Single-alternative output
> is byte-identical (goldens unchanged). **`E0186`** (`scope::check_create_sat`): a `create` on a scoped model
> whose mutation's `scoped …` set ⊇ no alternative → the create can't auto-set a full alternative; co-fires
> with `E0185`, skipped for `unscoped`. A dedicated OR (`@scope Page`/`@scope Author`) + AND (`@scope Page,
> Author`) fixture (codegen `tests/dml.rs`/`tests/mutations.rs` + sema `tests/check.rs`) proves per-callable
> predicate divergence and the E0185/E0186 triggers. `RModel.scope` retained (shard key D33 + E0181 guard).
>
> ✅ **Iteration 3 (editor surface + snapshot + UI scrub) landed as D50 — Track G complete.** Facts/LSP:
> go-to-def from `@scope Name` / `scoped Name` → the `scope` decl (`collect_scope_refs` + `definition_at`,
> the D43 twin), and hover on the decl or any ref describing the contract (`FactKind::Scope`, hover-only —
> inlay skips it). `schema.snap` now serializes scopes: top-level `scope <Name> (<col>: <Type> = $ctx.<f>)`
> decls + per-table `scope=(Name, …)` DNF groups (`Snapshot.scopes`/`TableSnap.scope_alts`), round-trippable,
> so an offline diff detects a scope added/dropped/renamed/retyped or a model joining/leaving (a no-DDL
> `Step::ScopeChange`; init stays create-only). Commerce golden re-blessed. UI scrub: every editor-facing
> string in `based-facts`/`based-lsp` stripped of `D<n>`/principle/`.md` refs (the `$ctx` "(D4/D5)" hover
> leak is gone and now accurate), guarded by a regression test. Scope **rename** deferred to the C4 rename
> iteration (needs the full reference-site index).
>
*Spec settled in **D46** (named) + **D47** (multi-scope) + rewritten auth.md Handle 2 + grammar
(`scope_decl`, `scope_deco`, `scoped_clause`).* Scope is a first-class **named** declaration referenced by
name on both sides — the model (`@scope Name`) and every callable that touches it (`scoped Name`), the
escape hatch `unscoped("reason")` unchanged. **`@scope` is repeatable (D47): commas within one decorator
are AND (one alternative), stacked decorators are OR (alternatives) — a DNF; a callable confines by a set
⊇ one alternative.** Motivation: a scope contract this important must be **written, not implied**
(principle 2) — the old `@scope(pred)` inferred the `$ctx` type per callable (D4/D5) and only *showed* it
as an editor hint, the exact anti-pattern the user objected to. Implementation iterations (top-down; each
its own `cargo test`/`fmt`/`clippy`-green commit):
  - G1. **Parser + AST.** New `Decl::Scope` (`scope Name (col: Type = $ctx.field, …)`, grammar
    `scope_decl`); the `@scope Name[, Name]*` bare-name model-decorator form, **repeatable** (grammar
    `scope_deco` — distinct from the generic parenthesized decorator, like `@index barcode`; a model
    carries a *list* of `@scope` alternatives, each a comma-separated axis conjunction); the callable
    `scoped Name[, Name]*` clause (grammar `scoped_clause`) alongside the unchanged `unscoped_clause`;
    `scope`/`scoped` added to the positional-keyword set (D8). Golden/unit parser tests (single, AND, OR).
  - G2. **Sema — DNF alternatives + errors + coherence.** Resolve `scope` decls (predicate = the D32
    restricted conjunction, now checked at the decl site → `E0180`); resolve `@scope` / `scoped` refs
    (`E0183` unknown scope); check each `@scope` model carries the named scopes' columns at a conforming
    type (`E0184`, per decorator). Model the `@scope` stack as a **DNF set of alternatives** (each a set of
    scope axes). Enforce the **required-declaration rule** — a scoped callable with neither `scoped` nor
    `unscoped` is `E0182`; enforce the **superset-of-an-alternative rule** — the `scoped` axis set must ⊇
    ≥1 alternative of *each* touched scoped model (root + D34 joined reaches), else `E0185` (revised: too
    few axes for any alternative, or an axis no touched model declares). New **`E0186`** — a `create` whose
    auto-set can satisfy no alternative (or a required non-null scope column with no `$ctx` value). Source
    the scope field's `$ctx` **type from the decl** (ending the D4/D5 inference for it); `E0161` coherence
    becomes structural for the scope field (still fires for non-scope `$ctx`). `E0181` (create assigns scope
    col) + `W0106` (stale unscoped) carry over. Duplicate `scope` name via the general duplicate-decl path.
    Retarget `RModel.scope`/`scope_terms()` onto the named decl → a `Vec` of alternatives.
  - G3. ✅ **done (D49). Codegen + runtime — per-callable alternative injection.** Sema resolves
    `RQuery`/`RMutation.scope_inject: Vec<ScopeInject>` (per touched scoped model, the chosen axes' terms —
    `scope::resolve_inject`), threaded into `Select` (`with_scope_terms`); `Select::scope_where(alias, model)`
    builds the ANDed `col = :ctx_<field>` and replaces every prior read of `RModel.scope`/`scope_terms()` in
    the SQL path (root `WHERE`, joined `ON` via `scope_join_pred`, create auto-set, write guards, restore, D12
    re-select). **`E0186`** (`scope::check_create_sat`): a `create` whose mutation's `scoped …` set ⊇ no
    alternative of the created model → can't auto-set a full alternative; co-fires with `E0185`, skipped for
    `unscoped`. Single-alternative single-axis SQL is byte-identical (goldens unchanged — the regression
    proof). OR/AND fixtures (codegen + sema) prove per-callable predicate divergence. `RModel.scope` retained
    (shard key D33 + E0181 guard). Binds stay `:ctx_<field>`.
  - G4. **Commerce migration to the new syntax.** Rewrite `spec/examples/commerce` `.bsl`: add
    `scope Tenant (org: Org = $ctx.org)`, put `@scope Tenant` on `Order` (and a second scoped model —
    OrderItem gains `org: Org` — to exercise multi-model composition + joined-`ON`), and add an **OR**
    example (a model with two stacked `@scope` alternatives — e.g. `Post` by page-or-author) to exercise
    the DNF path; annotate every callable with the right `scoped …` alternative (or `unscoped(…)` for the
    admin lookup). Re-bless conformance goldens + the sema commerce-clean test. **Do this only once G1–G3
    land** (the parser must accept the new syntax first — this iteration deliberately left commerce on the
    old `@scope(pred)`). ✅ **Core done (D48):** commerce carries `scope Tenant` / `@scope Tenant` /
    `scoped Tenant` and checks clean. The **DNF proof** (OR + AND) is a dedicated codegen+sema fixture (D49),
    not commerce — adding an OR model to commerce is optional polish, still open.
  - G5. ✅ **done (D50). Facts/LSP + migration snapshot.** Facts/LSP: go-to-def from `@scope Name` /
    `scoped Name` → the `scope` decl (`collect_scope_refs` + `Snapshot::definition_at`, the D43 twin), and
    hover on the decl or any ref (`FactKind::Scope`, span-anchored at the decl + every `@scope`/`scoped`
    site; hover-only — the LSP inlay skips it, since a scope is *written*, not derived). **Scope rename
    deferred** to the C4 rename iteration (needs the full reference-site index). **Scrubbed every
    editor-facing string** in `based-facts`/`based-lsp` of `D<n>`/principle/`.md` refs (the `(D4/D5)` `$ctx`
    hover leak is gone and now accurate — the scope field is declared), guarded by a regression test.
    Migration snapshot (`based-codegen::migrate`, extends D39): `Snapshot.scopes` serializes top-level
    `scope <Name> (<col>: <Type> = $ctx.<f>, …)` decls (sorted, before tables); `TableSnap.scope_alts`
    records each table's DNF as `scope=(A, B)` header groups (one per alternative). Both round-trip; a scope
    change surfaces as a no-DDL `Step::ScopeChange` (advances the snapshot so `verify` drift stays honest;
    init stays create-only). Commerce golden re-blessed (`scope Tenant (org: Org = $ctx.org)` + Order's
    `scope=(Tenant)`); multi-alternative OR round-trip/diff unit test added.

**Track D — deploy + keep-proven (DoD #4, last).**
  - D1. Dockerfile / image for `based serve` (health/readiness + graceful drain behaviour already
    done, D26 — this is packaging). D2. CI running the real-DB suites (A) + example builds (B) +
    extension build (C) + the migration apply tests (E4) so the whole thing stays green.

**Track F — source hygiene pass (quality, cross-cutting; standalone value, off the DoD critical
path — worked when it won't preempt A/B/D/E).**
  - F1. **Finalize comments across all source.** Sweep every `crates/**/*.rs` and rewrite build-time /
    WIP narration ("here's what I'm building", running commentary on construction-in-progress) into
    clean, **brief** what+why comments matching surrounding density. `sqlite.rs` is the known offender —
    do it first, then the rest of the workspace. Source must read as finished source, not a scratch pad:
    narration reads as unfinished and leads humans *and* agents off task (invites re-litigation, buries
    intent). Move TODOs out of code into PLAN.md / the relevant roadmap `.md` unless a TODO is genuinely
    must-do/blocking (then it may stay inline, terse). Comment-only, so it gates on `cargo fmt --check`
    + `cargo clippy` (tests unaffected). The standing rule is recorded in Conventions below so new code
    holds the bar from the start.


---

## based-sema — deferred resume points (all resolved)

The sema build-out's resume list. Every item below landed; kept for the delivered-shape
detail (error codes, test counts, the exact resolution taken).

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
   contributes nothing to inference today. ~~Also deferred: emitting the per-callable
   `Ctx` type in the client.~~ ✅ **done (D30)** — each callable's `ctx_requires` bag
   is now a typed `<Name>Ctx` struct the generated Rust client method takes (a public
   callable takes `()`); the `Transport` carries it as request context. Still deferred:
   `$ctx` passed *as a filter arg* (arg/usage typing, D14).
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

---

## Milestones ahead (post-sema) — delivery log

The M2–M6 milestone narration: DDL, query/mutation SQL, client codegen, LSP, and the
runtime, each with its delivered/deferred detail as written at ship time.

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
    left). `@scope` (auth.md) rides the same path — on the root `WHERE` **and** every
    joined scoped model's `ON` (D34), so a relation reach into another tenant is
    filtered too. Conventions recorded in **D11**/**D34**.
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
    aggregation / a second query; skipped in projection); keyset cursor
    comparison + opaque cursor encoding (runtime concern — base SELECT is ORDER+LIMIT).
    (`@scope` injection **resolved, D32** — uniform single-owner filter, create auto-set,
    `unscoped` escape hatch; `@tenant` was removed, folded into `@scope`, D19.)

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
    same-name scalar → its type); defaulted/optional params → `Option<T>`. `$ctx` is never an input —
    it is a **separate typed `<Name>Ctx` method argument** (D30): a struct of the callable's
    `ctx_requires` bag (relation → `Uuid`, scalar → its type), or `()` for a public callable.
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
  `detail` as tooltip), **hover** (the fuller "why" for any fact whose span
  covers the cursor), and **go-to-definition** (Cmd+click a model/type reference →
  its declaration, cross-file; D43). `LineIndex` does faithful UTF-16 position mapping
  (LSP's default). Tests: `based-lsp/src/compile.rs` unit tests (position round-trips
  incl. multibyte; `compile` over commerce; go-to-def cross-file). Smoke-tested
  end-to-end over the JSON-RPC wire.
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
  - **Go-to-definition ✅ done (D43); completion / rename still deferred.**
    General IDE ergonomics, not derived-fact surfacing, so principle 8 neither
    requires nor forbids them — an ordinary product call, sequenced after the VS Code
    client. Go-to-def landed as a Track C follow-up: `Snapshot` now retains the parsed
    `decls`, `Snapshot::definition_at(fid, offset)` collects every model/type-reference
    `Ident` across the AST (field types, opt-in inverses, shape `from`, query/mutation
    return + param types + `get`/`list` targets, write targets incl. nested `tx`,
    filter param types) and resolves the one under the cursor to its `Model`/`Shape`
    declaration's name span — cross-file, routed to the owning snapshot exactly like
    hover/inlay. Also shipped: **type-name syntax coloring** in the VS Code grammar
    (`editors/vscode/syntaxes/bsl.tmLanguage.json` `#types`: builtin primitives →
    `support.type.primitive`, PascalCase refs → `entity.name.type`; D43). *Still
    deferred:* completion + rename — rename needs the *reference-site* index (all uses
    of a symbol), a superset of the single-target resolution go-to-def uses.

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
    **The shard key is now bound to the resolved `@scope` `$ctx` field (D33):** each callable's
    `RQuery`/`RMutation.shard_key` (`RModel::shard_key_ctx_field`) records its target model's scope
    owner field, read off the *same* `@scope` that filters the row so routing and row-visibility
    can't drift; an `unscoped` callable has no owning shard. The listener (`http::resolve_shard_key`)
    pulls that field out of `$ctx` per request (`X-Based-Shard-Key` override retained), retiring the
    hand-set `--shard-key-field` flag.

*HTTP listener (`based serve`) — delivered (D21):*
  - **`based-runtime::http`** (feature `serve`) — the thin socket edge over `serve::dispatch`.
    A **sync bounded worker-thread pool** over the bounded connection pool (D20): N workers
    share one blocking `tiny_http::Server` (hardened lib, principle 7), each looping
    `recv → decode → dispatch → respond`. `based serve <root>` (CLI) loads the checked schema,
    builds the `ShardRouter`, and runs it (`--listen`, `--database-url` × shards / `BASED_DATABASE_URL`,
    `--workers`, `--pool-{min,max}`; the shard key is schema-derived per callable — D33 — so there is
    no `--shard-key-field` flag).
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

*Not started (next slices) — NOTE: the **Completion roadmap** near the top of this file is now the
authoritative ordering. The "deferred to the live-DB slice" / "not production-real until that lands"
language below is superseded — that work is **Track A** (concrete Postgres/MariaDB drivers + live
Docker-backed suites) and **Track D** (container image + CI), on the critical path, not blocked. The
detail below is retained as reference for what each entails:*
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
    retryable-on-failure / no-slot-on-bad-request) + 1 socket end-to-end. ~~*Deferred:* … rejecting a
    replayed key carrying *different* args~~ ✅ **done (D31):** the key now carries a **request
    fingerprint** (a stable FNV-1a hash of the request's args + `$ctx`, `Request::fingerprint`); a reused
    key on a *changed* payload is `KeyState::Mismatch` → `RunError::KeyReuse` → a non-retryable `422`
    `idempotency_key_reuse` (distinct from the retryable `409` an in-flight *same-payload* duplicate
    gets), never a silent replay of the first request (principle 1 — the dangerous case is loud).
    *Deferred:* a shared/durable store for multi-instance dedupe (needs live infra — same trait, and the
    stable FNV fingerprint is now ready for it), key TTL/eviction.

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
    straight in as a **typed `<Name>Ctx` argument** (D30) — no header dance, no side-channel bag —
    and the write `place_order` now decodes into a typed
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

---

## Track N — async-native pivot: delivery narration (N0–N3, D84–D89)

Moved from PLAN.md at N3 close-out (the live status there is now one line per item).
As written at the time; the per-decision record is D84–D89.

### Track N — async-native pivot (owner decision 2026-07-10; TOP PRIORITY)

**Strategic context.** The first real adoption target (the owner's workplace, the project's proving
ground) reviewed the pitch: the syntax landed well, **native async was repeatedly named the core
required feature**, streaming reads are wanted immediately, and they need the engine to plug into an
app's *existing* async connection pool (a `spawn_blocking` facade was judged a hassle by their backend
owner). The Rust web-backend market is effectively all tokio — axum and every runner-up run on it — so
async-native is the market, not a variant. Decision: recolor the execution core to native async, no
sync facade. The pure front end (parse → sema → codegen → plan/SQL lowering) stays sync and
runtime-free; coloring touches execution only. **All Track N work lands on a single long-lived branch
(`async-native`), merged to `main` only at demonstrated confidence** — full gate + all three live
suites + the examples green on the async core (owner, 2026-07-10). Worked in order.

- **N0. ✅ done (D84). Async architecture design — the elegance mitigation, settled before recolor
  code.** Every guarantee the sync design gave by construction is restated as an invariant with a
  named enforcement (type system > test > review). Owner-settled: **sqlx is the driver layer**
  (principle 7 — reuse hardened tx-drop/codec/pool/streaming machinery, delete our own driver stacks;
  executor/pool layer only — `Db`/`Backend` stay our traits, sqlx never appears in the trait surface);
  transactions become a consuming **typestate** (`begin(self) → Tx`, `commit(self)`;
  drop-without-commit = rollback-or-discard, an open-tx connection is never pooled — cancel-safety by
  construction, not vigilance); **`fetch` returns a row stream, always** (one-shot = collect at
  dispatch; N2 adds a wire surface, not a second execution path); the **coloring boundary is
  CI-enforced** (front-end crates provably tokio/sqlx-free via a `cargo tree` check); retry ×
  cancellation composes via per-attempt `Tx` (no double-write window; idempotency keys unchanged).
  Full design, trait sketch, invariant table, de-risk spike scope: **D84**.
- **N1. ✅ COMPLETE. Native async execution core (implements D84).** All four slices landed;
  N2 streaming is next.
  - ✅ **First step — the D84 de-risk spike** (`tests/sqlx_spike.rs`; live gate `ci-live-sqlx`):
    all three dialects codec-faithful through sqlx. Decimal feature = **`bigdecimal`**
    (`rust_decimal` silently truncates past ~28 digits, disqualified; `pg_numeric` stays, decoding
    sqlx's byte-exact raw numeric); Postgres binds must be **native-typed** — sqlx's all-binary
    parameters kill the coerce-wire-text trick, so `SqlValue` grows typed text-riding variants;
    MariaDB-via-MySql-driver confirmed (binary-charset uuid/json decode + `CLIENT_FOUND_ROWS`
    affected-rows quirk noted). Full findings: D84 addendum.
  - ✅ **The bulk recolor shipped.** Traits are the D84 shapes — `DbRead` (stream-only `fetch` +
    `execute`) / `Db` (`begin(self)` → `Tx`) / `Tx` (`commit(self)`, drop = rollback) / async
    `Backend` — with `dispatch`/`run_query`/`run_mutation`/`migrate apply` recolored on top
    (dispatch now owns checkout-per-call: it takes `Backend` + shard key). `SqlValue` grew the
    typed text-riding variants (uuid/timestamp/date/decimal); the planner types every bind site
    from the schema (params via their bound column, `$ctx` via inference, gen-ids as uuid, keyset
    cursor re-binds via the sort columns' primitives threaded through `LoweredQuery.keyset`); raw-SQL
    params stay text binds. All three hand-rolled driver stacks retired for sqlx 0.9
    executors/pools (statement timeouts via `after_connect`; `acquire_timeout` →
    `PoolExhausted` fast-503; deadlock retry = fresh checkout + fresh `Tx` per attempt; `pg_numeric`
    survives decode-only on raw bytes). `based serve` moved tiny_http → axum (healthz/readyz/drain
    kept; the worker-count knob retired — the pool is the concurrency ceiling). The `RefCell`
    `Engine` retired for a `Send + Sync` checkout-per-call handle over `Arc<dyn Backend>`. The
    generated client + `Transport` are async; the CLI wraps at `#[tokio::main]`; `MockDb` implements
    the async traits (Clone, shared state, drop-records-rollback). Execution tests are
    `#[tokio::test]`; the three quickstarts are async-integrated, minimal (the SQLite one lost its
    rusqlite plumbing — `SqliteBackend::open` is the whole wiring). The coloring boundary is
    CI-enforced: `make ci-coloring` (in `ci-workspace`) walks `cargo tree` for every front-end crate
    and fails on tokio/sqlx/futures/axum. **`make check` green** — full workspace suite + fmt +
    clippy + live MariaDB/Postgres suites + all three quickstart scenarios on the async core.
    Landing the gate surfaced + fixed two real recolor bugs (D84 implementation notes): the drain
    window (`/readyz` must observably 503 before the axum listener stops accepting) and the keyset
    `id` tiebreaker binding as uuid for a model that declares `id: text`.
  - ✅ **Cancel-safety acceptance gate (I2) shipped** (`tests/cancel_safety.rs`; runs in
    `check-fast`). A gate wrapper numbers every driver-seam op on the mutation path (checkout,
    begin, each execute, the re-select fetch, commit) and parks the future at each — once just
    *before* the op, once just *after* it completes; the test drops it there against a live
    file-backed SQLite (single-connection pool) and asserts: all-or-nothing row state (writes
    survive only a drop after the completed commit — in full), the pooled connection is in
    autocommit (explicit `BEGIN IMMEDIATE` probe), and the same pool serves the next mutation
    green. Await points *inside* one driver call are sqlx's own cancel-safety (delegated,
    principle 7). The gate caught + fixed a real bug: a cancelled **keyed** mutation stranded its
    idempotency claim `InFlight` forever (every retry → 409 Conflict); `run_mutation` now holds
    the claim in an abandon-on-drop guard, disarmed only once the response is recorded (D84 notes).
  - ✅ **BYO-pool seam shipped (the design-partner embed — the last N1 item).**
    `ShardRouter::from_pool(MySqlPool)` / `PgRouter::from_pool(PgPool)` /
    `SqliteBackend::from_pool(SqlitePool)` build the `Backend` over a caller's *existing* sqlx
    pool (cheap-cloned; one physical shard), sharing the codec/tx path with the URL-built
    constructors. Contract (D84 notes): **their pool, their settings** — the engine installs
    nothing on a supplied pool (the session statement timeouts our constructors apply ride
    `after_connect`, a builder-only hook; reconfiguring sessions the app's own queries share
    would be wrong anyway); pool-exhaustion fast-503 classification + deadlock retry work
    unchanged. Proven live on MariaDB + Postgres (`byo_sqlx_pool_backs_the_engine`: the app's
    own sqlx queries and the engine's scoped read + transactional mutation interleave on one
    pool) plus a SQLite unit twin.
  - Gate held throughout: full workspace suite + fmt + clippy + all three live-DB suites green.
- **N2. ✅ COMPLETE. Streaming reads (claims N1's payoff immediately).** The driver seam already
  streams — `fetch` returns a sqlx-backed row stream on all three dialects (D84 decision 3); N2
  surfaced it end to end: signature form → sema → runtime dispatch → NDJSON wire → generated
  client → OpenAPI, with the acceptance gates live. N3 is next.
  - ✅ **Spec/design slice (D85, `spec/syntax/streaming.md`).** Opt-in is the signature return form
    `-> stream Shape` (grammar extended; contract lives where the client surface is generated from);
    wire = NDJSON envelope-per-line with a mandatory terminal `done`/`error` line (no terminal line
    = truncation = transport error; pre-body failures keep real statuses); client = same-named
    method returning `Result<RowStream<Shape>, ClientError>` with per-item `Result`, drop = cancel;
    `page` forbidden (E0201), `get`/mutations can't stream (E0200/E0202), everything else (filters,
    sorts, shapes, scope, soft-delete, index lint) composes unchanged on the single read path.
  - ✅ **Front-end slice.** `RetType.stream` parses (`stream X[]` is a parse error — `stream`
    already means many), sema infers `list` and rejects the three misuses (E0200 get / E0201 page /
    E0202 mutation), the flag rides `RQuery.stream` (with `many = true`, so SQL lowering is the
    `[]` form, untouched); fmt round-trips, LSP hover/completion/facts surface the form;
    conformance goldens (parser positive + no-brackets negative, sema positive + errors bundle).
  - ✅ **Runtime streaming dispatch + NDJSON wire.** `run_query_stream` (plan → owned
    `ShapedStream` of shaped rows over the checked-out connection; drop = cancel, connection back
    to the pool) + `dispatch_stream` (the streaming twin of `dispatch`: pre-body failures are the
    ordinary `WireResponse` with real statuses) + `Engine::call_stream` (in-process door). The
    axum edge branches on `Compiled::is_stream_query`: `200` + `application/x-ndjson`,
    `{"row":…}` per line, mandatory terminal `{"done":{"rows":N}}` or in-band `{"error":…}`;
    non-stream traffic byte-for-byte unchanged. Proven mock + live SQLite (rows in sort order,
    terminal framing over a real socket, mid-stream error line, drop-mid-stream returns the
    single pooled connection healthy).
  - ✅ **Generated client + OpenAPI.** A `-> stream` query's method keeps its name and returns
    `Result<RowStream<Shape>, ClientError>` (`RowStream<O>` = boxed `futures_core` stream of
    per-item `Result`; drop = cancel); `Transport` gains `call_stream` beside `call`; the module
    emits `decode_ndjson` — the one framing decoder (line reassembly across chunks, in-band
    `error` → typed `Err` item, no terminal line = truncation `Err`, `done.rows` checksum
    enforced) any HTTP transport feeds its byte stream through — and the embedded bridge
    implements the streaming door over `Engine::call_stream` (typed items, no NDJSON
    round-trip). All of it emitted **only when the schema declares a stream query**, so a
    non-streaming schema's module (and dependency set) is byte-identical to before — the three
    quickstart clients needed no regeneration. OpenAPI: the stream query's `200` is
    `application/x-ndjson` with the row/done/error one-of line schema; pre-body failures keep
    the JSON error responses.
  - ✅ **Acceptance gates.** Live MariaDB + Postgres: the full `based serve` NDJSON body over a
    live router (rows in sort order + terminal `done` count); live Postgres: a raw-SQL
    divide-by-zero firing mid-pass arrives as the in-band `error` line after delivered rows.
    Generated-client-over-real-HTTP suite (`streaming_client.rs`, reqwest transport): typed
    rows, the in-band error as a typed `Err` item (code `database_error`, 503), pre-body 400 as
    the outer `Err`, and a real socket cut mid-body → transport `Err`, never completion (plus
    pure chunk-stream decoder twins). The I2 cancel gate grew the streaming twin: drop a stream
    mid-pass on the single-connection pool → connection back in autocommit, next mutation green;
    the embedded typed client proves the same drop-release end to end.
- **N3. Flagship axum example + syntax appeal pass (the re-pitch artifact).** A nontrivial
  `examples/axum-…` service — multiple routes, auth-derived `$ctx`, scoped multi-tenancy, a streaming
  endpoint, migrations, the typed async client end-to-end — that reads like the app a workplace backend
  dev would actually write, at quickstart-DX polish (no plumbing). Paired with a deliberate **syntax
  appeal pass** over every surface the example shows (the `.bsl` files first, then README + client call
  sites): the syntax is what landed in the pitch — polish it for first-look impact, not just
  correctness. The example *is* the pitch. **Coverage policy (owner, 2026-07-10):** the three
  quickstarts stay largely as they are — async-integrated but minimal — and the axum example is the
  **total-feature-coverage** vehicle: every language/runtime feature demonstrated somewhere in it, so
  feature-coverage growth lands in one example instead of three.
  - ✅ **Design gate (D86).** Domain = a multi-tenant **support desk** (`examples/axum-helpdesk`;
    not commerce — the flagship must show the language generalizes), dialect = **Postgres only**
    (the modal axum+sqlx pairing; the strictest bind path N1 built). Architecture: the app embeds
    the engine — its own sqlx `PgPool` → `PgRouter::from_pool` (the BYO-pool seam, demonstrated) →
    `Engine` in axum state; auth middleware resolves `Authorization: Bearer` through the typed
    client itself (`session_by_token … unscoped("auth: …")`) into per-request `Ctx { org, user }`.
    ~12 routes across three audiences (requester portal / agent desk / ops+finance export). Full
    feature→site coverage map + the deliberate exceptions (`based serve`/image, legacy affordances
    `(column …)`/`@table`/`on:`, `shape full`) recorded in D86. **Syntax-appeal audit verdict: zero
    grammar changes** — the drift it found was worked-example/prose level and is fixed (commerce
    `UserRef` shape-name drift; pagination.md pre-grammar example forms); accepted-as-is list with
    rationale in D86.
  - ✅ **N3a (prereq). Ordered to-many nests (D87).** A nest's array now follows the sort
    cascade for the traversal — relation `@sort` > child model `@sort` > unspecified — as an
    ORDER BY *inside* the JSON aggregate on all three dialects (one `json_array_agg` seam;
    subquery shape + single read path unchanged, so streams get it free). Zero new syntax
    (field `@sort` already parsed + sema-checked; `RMember` now carries it into lowering).
    Proven by per-dialect codegen assertions + live out-of-order seeds coming back sorted on
    SQLite (normal gate), MariaDB, and Postgres (both tiers in one response each).
  - ✅ **N3b (prereq). Two seams (D88).** (i) `guard` runs: `Guards::new().register(name, async fn)`
    → `Engine::with_guards` (build fails naming any unregistered declared guard; `Engine::new`
    panics on a guarded schema; `based serve` refuses one); `dispatch` — the one core both doors
    run through — invokes the guard before the write/idempotency-claim/arg-validation; deny →
    `403 guard_denied` with the guard's mandatory reason. Proven mock + live SQLite (a guard that
    reads the DB itself: close allowed once, denied after its own write). (ii) Every generated
    mutation method gains a `<name>_with_key(input, ctx, key)` twin over a new required
    `Transport::call_with_key` — `Idempotency-Key` header on HTTP, `Engine::call_with_key`
    embedded — emitted only when the schema declares a mutation (query-only modules byte-identical).
    Replay proven through the typed client over a real socket + in-process (one tx, identical
    bodies). OpenAPI documents the key header, 409/422, and the guard 403. Committed generated
    clients regenerated (gate-enforced).
  - ✅ **N3c. Schema + migrations + client.** The helpdesk `.bsl` (by-domain layout, 26 callables),
    `0001_init` + `0002` `@was` rename (verify green), checked-in verbatim `src/client.rs`
    (`--embedded`), seed via the client's own mutations (D63 pattern; prints demo bearer tokens;
    ran green against live Postgres incl. D87 out-of-order time entries + scope/guard/tx probes).
    Being the first real consumer of the whole surface, the slice surfaced + fixed six engine
    bugs: multi-alternative `@scope` ctx/exemptions/lints derived from the *first* alternative
    instead of the callable's chosen one (`inject_ctx_reqs`); bare `@sort(name)` silently dropped
    (fmt's own canonical spelling!); enum-annotated params typed/bound as `Id<entity::…>`/uuid;
    enum param *defaults* bound by variant name, not wire value; absent optional to-one nests
    decoded as objects-of-nulls (now JSON `null` via a `__present` probe / CASE collapse); decimal
    inside JSON aggregates rode as a JSON number (now text-cast, keeping the string contract);
    plus `has`/`in` in per-param bindings lowering non-dialect-aware. Coverage-map deviations to
    resolve at N3e: `in` has no value-list form (search uses `not`/`or` instead) and raw
    whole-query bodies don't exist (workload report uses raw correlated-subquery *values* —
    arguably the better raw.md story).
  - ✅ **N3d. The axum service.** `src/{main,app,auth,routes}.rs`: the D86 architecture live —
    the app's own sqlx `PgPool` → `PgRouter::from_pool` → `Engine::with_guards` in axum state;
    bearer middleware resolving tokens through the typed client (`session_by_token`) into a
    request-extension `SessionCtx` (role-gates the desk/ops routes); ~20 routes, each handler one
    typed call; `Idempotency-Key` on `POST /tickets` via the `_with_key` twin; the export
    re-served as NDJSON (row/done/error framing mirroring the wire) from `RowStream`; one
    `ApiError` passing the engine's status + stable code + envelope through untouched. Close
    policy chosen for `caller_can_close`: *only a resolved ticket, visible in the caller's
    workspace, can be closed* — host code on the app's own pool (the D88-proven pattern), denying
    by default when it can't verify. Live gate: `make ci-example-helpdesk` (in `ci-examples`) —
    reset → migrate → seed → boot in-process → drive the whole surface over real HTTP (401s,
    disjoint tenants, role 403, search/status/bad-cursor 400, queue, ordered nests, cross-tenant
    404, keyed replay + 422 reuse, guard 403→allow, archive/restore, AND-scope drafts, tags/
    per-param bindings, workload raw SQL, unscoped offset admin, NDJSON checksum + dropped-stream
    recovery). **Notes for N3e (library gaps hit, each wants its own slice):** (i) a guard
    calling the typed client over its *own* engine deadlocks — `Engine::call` holds the id-gen
    lock across dispatch, so the D88 doc's "or call the typed client itself" only works against
    a second engine; narrow the lock. (ii) `mutation … -> Shape { hard delete … }` is
    undecodable through the typed client (wire returns `{}`, client expects the shape) — the
    purge-comment route is omitted; sema should reject the pairing or the re-select should run
    pre-delete. (iii) `with count`'s `total` is dropped by the generated `Page<T>` (no field) —
    the admin route serves rows+cursor only. (iv) a zero-row update (e.g. cross-tenant id) is
    a `200 null` body → typed decode error, deserves a `not_found` outcome. (v) `~` takes a
    verbatim LIKE pattern — the handler wraps `%…%`; fine, but worth a docs line.
  - **N3e. README + re-audit + CI.** The re-pitch README (walks the `.bsl` surfaces first), a final
    first-look appeal pass over the real artifact, example wired into CI (`make check` already
    runs it via `ci-examples`); plus the N3c/N3d deviation list above.
