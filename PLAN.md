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
- **Gate before commit — two tiers, one command each (velocity rule, owner 2026-07-10).** While
  iterating, run **`make check-fast`** (fmt + clippy + full workspace tests; no DB). Before committing
  an execution-touching change (runtime/codegen/drivers/examples), run **`make check`** — it starts
  fresh throwaway DBs itself (Docker via OrbStack) and runs check-fast + both live suites + all three
  example scenarios; do NOT hand-assemble the cargo/docker sequence across multiple steps. A
  front-end-only change (parser/sema/fmt/LSP/docs) may commit on `check-fast` alone. Never commit red.
  A driver/live slice is not "done" until `make check` is green — live against a real server, not
  compile-verified.
- **Commit style.** On the current working branch (no push, no PR): first line
  `m6: <desc> (D<n>)`, short body, ending with the trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Update this file
  and `decisions.md` in the *same* commit as the code.
- **Sequential only.** Each iteration commits before the next starts (so the next reads
  updated state). The coordinator NEVER touches the repo/git while a subagent is running —
  they share one working tree + index.
- **Pause** after 3 items in a batch, or when a subagent hits a genuine blocker (it stops
  WITHOUT committing and reports), or when the unstarted items are exhausted.

**Where things stand (as of D66):** the architecture milestones (M2–M6) are done, all three target
dialects (SQLite/MariaDB/Postgres) clear the real-DB bar — including symmetric live coverage of
keyset/offset pagination + soft-delete/restore (D59) — **Track L — the language — is feature-complete**
(nested/relation projection D55/D57, keyset-cursor pagination D56, update/delete declared-shape re-select
D58), and **Track B — example projects (DoD #2) — is now COMPLETE**: one copyable, runnable worked project
per target DB, all three green via `cargo run` (SQLite D60; MariaDB + Postgres against live Docker servers
D61, which also surfaced + fixed a real Postgres binary-format result-decode bug). A complete, functional,
usable BSL covers ordinary GET/PUT end-to-end (get/list, filters, sort cascade, offset + keyset pagination,
create/update/delete/restore/`tx`, soft-delete, `@scope`, raw SQL, idempotency, declared-shape read-back on
every surviving write), and **DB-verification hardening is now done (A4/D65)**: statement timeouts,
deadlock-retry, and pool-exhaustion→fast-503 are built into the drivers + `PoolConfig` and proven live
against `mariadb:11.4`/`postgres:16`. **Track D — deploy — is now COMPLETE:** D2/D3 CI (D64) plus the
`based serve` **container image (D1/D66)** — a multi-stage `docker/Dockerfile` (dialect-aware serve, env
config, `/healthz` HEALTHCHECK, graceful drain), built + verified serving live `postgres:16` end-to-end,
smoke-booted in CI via `make ci-image`. **Track E — migrations — is now COMPLETE (E5/D67):** `@was`
data-preserving renames (snapshot-authoritative, per-dialect `ALTER … RENAME`), the offline
schema-vs-migrations drift diagnostic (LSP `W0108` + `based migrate verify`), and the `raw(dialect)` up
step, all proven live — so **DoD #5 is fully met** and **every DoD item is now met**. The **VS Code / LSP
feature-parity fill-in (C4) is now complete (D68):** folding + selection ranges land, code actions are
declined as out-of-scope (documented), `language-configuration.json` is verified — so DoD #3's
feature-parity fill-in closes. **Every roadmap item is now complete** — the last one, the cross-cutting
source-hygiene pass (**F1/D69**), swept every `crates/**/*.rs` and confirmed the source reads as finished
(only a few residual narration bits tightened; prior incremental hygiene had kept it clean). **The project
was fully done per the Definition of Done** — every DoD item met, no open features, no open hygiene.
Post-completion, **Track L4 — named nested projection (shape-in-nest) — is now done (D79)**: a nest may
reference a named shape (`placed_by -> UserRef`) for consumer-side type identity.
**2026-07-10 strategic pivot: Track N — async-native core → streaming → flagship axum example — is now
TOP PRIORITY (owner decision, design-partner driven); Track T resumes after N1.**
Batch-by-batch history is in `PLAN-archive.md`.

## Definition of Done (the product is complete when…)

Acceptance criteria. Everything in the Completion roadmap serves one of them. Status is the
current-truth summary; the evidence (which D# proved it) is in the archive.

1. **Proven against every target DB.** Each dialect the codegen emits has a concrete `Db`/`Backend`
   driver **and** a live integration suite running the *verbatim* `based gen sql` output against a
   **real server (Docker)** — not compile-verified, not MockDb. Per-DB coverage: get/list,
   `$ctx`-scope filtering (row + joined-`ON`), write + declared-shape re-select under one tx
   (read-your-writes), pagination, soft-delete/restore, idempotency dedupe, `Backend::ping`.
   **✅ Met** — SQLite (D27), MariaDB Docker (D35), Postgres Docker (D38). Keyset + offset pagination
   and soft-delete/restore read-back are now proven live on **all three** dialects (SQLite D56/D58,
   MariaDB + Postgres D59), so the per-DB coverage is symmetric. The A4 *hardening* items — statement
   timeouts, deadlock-retry, pool-exhaustion→fast-503 — are also **done and proven live (D65)**.
2. **A real, copyable example project per target DB.** A standalone Rust project (in-repo, **outside**
   the workspace, under `examples/`) consuming the generated client + runtime against a live DB — the
   thing a user copies to start. Builds in CI, doubles as an end-to-end smoke test. **✅ Met** — one
   worked project per dialect, each running the full create→read-your-writes→list/scope→paginate→
   soft-delete/restore scenario green via `cargo run`: `examples/sqlite-quickstart` (D60, bundled
   SQLite), `examples/mariadb-quickstart` + `examples/postgres-quickstart` (D61, live `mariadb:11.4` /
   `postgres:16` via Docker; the Postgres slice surfaced + fixed a real binary-format result-decode bug).
3. **A functional, installable VS Code extension.** Packaged (`.vsix`), registers `.bsl`, launches
   `based-lsp`, surfaces diagnostics + inlay hints + hover + go-to-def + symbols + completion.
   **✅ Met (D36 installable; Track C4 feature-parity complete, D68).**
4. **Deployable + kept-proven.** A container image / Dockerfile for `based serve`, and CI running the
   real-DB suites + example builds + extension build so none of it rots. **✅ Met (D64 + D66):** the
   `based serve` container image (`docker/Dockerfile`, dialect-aware serve, env-configured, `/healthz`
   HEALTHCHECK, graceful drain) built + verified serving live `postgres:16` end-to-end (D66); portable
   `make` targets (workspace, live MariaDB/Postgres, examples, extension, `ci-image`) + a thin
   `.github/workflows/ci.yml` example, all proven green locally against live `mariadb:11.4`/`postgres:16`;
   the live suites honor an external `TEST_*_URL` (CI service container).
5. **Schema evolution: migration generation.** A `.bsl` change produces a reviewable, editable
   migration you can safely apply to an existing DB — not just from-scratch DDL. **✅ Met** —
   spec (E1), snapshot + diff (D39), per-dialect render (D41), apply + `_based_migrations` ledger +
   status/verify + raw-SQL `down.mig` (D42), and **`@was` renames + offline drift diagnostic +
   `raw(dialect)` up step (D67)** — all proven live. **✅ Fully met** — a `.bsl` change (including a
   data-preserving column/table rename) produces a reviewable, editable migration you safely apply.

Deferred items (durable multi-instance idempotency store, shutdown grace deadline, incremental LSP
sync, `^^` multi-level back-refs) stay deferred — worked only if they land on the critical path or a
user would notice their absence. **Promoted off this list by the 2026-07-07 audit:** *nested shape
sub-objects + relation-array projection* → **Track L1** (core language surface, not optional — no query
can return a related object/array without it); *self-ref join aliasing* → folded into L1 (the flagship
`User.invited_users` case is self-referential and forces it); *rename* → done (D53).

## Completion roadmap (ordered for velocity)

**Priority order — largest value, closest-to-done first: Track L (finish the language) comes before
everything.** A complete, functional, usable BSL — all normal-workday GET/PUT query + shape surface,
working end-to-end — is the product's spine; DB-verification (A), examples (B), and deploy (D) are
*followups* to it, because you cannot verify or ship a feature that isn't built. The **independent
non-feature tracks — C (extension) and E (migrations) — share no files with L and *may run in parallel
batches* alongside it**; **A and B are subordinate and largely gated on L** (A's keyset + richer-shape
live coverage can't be proven until L builds them). **D** closes it all out (its CI must cover E).
Order *within* a track is top-down. Done items are one-liners with their D#; open items carry full
resume context. Delivery detail: `PLAN-archive.md`.

**Track L4 — named nested projection in shapes. ✅ COMPLETE (D79).** A nest may reference a top-level
shape by name — `placed_by -> UserRef` — giving the projection one nominal identity every fetch site and
`db→props` mapper can name (the client emits `placed_by: UserRef`/`Vec<UserRef>` instead of a per-parent
anonymous struct; OpenAPI `$ref`s the named schema). The reference is a pure column-list expansion:
byte-identical SQL to the inline nest, child scope/soft-delete governed by the nest context; the
referenced shape's `from` must equal the relation target (`E0133`; unknown shape `E0132`, reference
cycle `E0134`). Verified live on SQLite (to-one + to-many). Worked into `spec/examples/commerce`
(`UserRef`/`OrderDetail`). The (a)-vs-(b) design fork (reference a decl vs name an inline nest) resolved
to (a); (b) stays possible later sugar — detail in D79.

**Track L — language feature-completeness (TOP PRIORITY, critical path, DoD-spanning). ✅ COMPLETE
(L1 D55/D57, L2 D56, L3 D58).** The read/write *core* is built and proven live — get/list,
`where`/operators/named-filter inlining, the sort cascade, offset pagination,
create/update/delete/restore/`tx`/single-level `^`, soft-delete, `@scope` injection, raw SQL,
idempotency — and the three *specified, normal-workday* gaps the 2026-07-07 audit found (below) are all
closed: nested/relation projection, keyset pagination, and update/delete declared-shape re-select. A
complete, functional BSL now covers ordinary GET/PUT end-to-end. The gaps, for the record:
  - **L1. ✅ done (D55 to-one + D57 to-many). Nested / relation projection in shapes.** To-**one**
    sub-objects (D55): `field { … }` over a Forward (or unique one-to-one Inverse) relation nests the
    target's columns end-to-end — SQL prefixes them under a `field.<col>` alias (reusing the reach-rename
    JOIN), the runtime `nest_row` reassembles the flat row, client/OpenAPI emit the nested type; recurses.
    To-**many** arrays (D57): `field { … }` over an Inverse collection (`OrderCard.items`, the
    self-referential `User.invited_users`) lowers to a **correlated subquery** in the SELECT list
    aggregating the child rows into a JSON-array column aliased `field[]` (per-dialect: SQLite
    `json_group_array`, MariaDB `JSON_ARRAYAGG`, Postgres `json_agg`, all three coalesced to `[]`); the
    child gets a distinct `s<n>_<table>` alias so a **self-ref** edge never collides with the outer row;
    the runtime parses the array string into real sub-objects; client emits `Vec<Sub>`, OpenAPI an array
    schema. Child soft-delete + `@scope` ride the subquery WHERE (D34). Nests compose to any depth
    (to-many-in-to-one, to-one-in-to-many). Verified live against SQLite (order→items with a soft-deleted
    child excluded + a childless parent → `[]`; self-ref `invited_users`). All 3 dialects unit-covered.
    Array element **order is unspecified** (portable JSON aggregation has no cross-dialect ordered form) —
    documented; an ordered form is a future refinement if a use case needs it.
  - **L2. ✅ done (D56). Keyset-cursor pagination.** A keyset `page` now walks the whole set: codegen
    emits the lexicographic "strictly after the cursor" `WHERE` (guarded by `:keyset_active`, a no-op on
    page 1) over the resolved sort keys + hidden `__keyset_<i>` cursor-basis columns; the runtime decodes
    the incoming `cursor` into `:keyset_<i>` binds, mints the next opaque cursor from the last row (checksum-
    validated, `cursor.rs`), and strips the hidden columns. Client/OpenAPI carry `cursor`/`offset` inputs.
    A non-offset `page` always gets the `id` tiebreaker (deterministic even with no `order`). Proven live
    against SQLite (paging full→full→short, tampered cursor → 400). Unblocks A4's live pagination coverage.
  - **L3. ✅ done (D58). Update/delete declared-shape re-select.** A mutation now reads its written
    row back in its declared shape even when it only *updates / soft-deletes / restores* it, not just on
    *create* (D12). The re-select is **where-keyed**: it reuses the surviving write's own `where` (+
    scope; + the soft-delete live predicate for update/restore, dropped for a soft delete so the
    tombstoned row still reads back), runs after the write inside the same tx (read-your-writes), and
    reuses the read side's `project_return` so nested to-one (D55) / to-many (D57) shapes work in a
    re-select. **Delete-shape resolution:** a **real DELETE** (plain-model `delete` / `hard delete`)
    removes the row, so there's no surviving row — those emit no re-select and return `{}`. Verified live
    against SQLite (an `update` returning the full `OrderCard { status, total, placed_by { name } }` with
    the new value, one tx). Incidental fix: SQLite UPDATE `SET` now emits a bare column (SQLite rejects a
    qualified `SET` column — was never live-tested before this update path existed).

**Track A — real-DB proof (DoD #1; A4 now UNBLOCKED — Track L, which it was gated on, is complete). NEXT
on the critical path.** *Mechanism: Docker (OrbStack).*
  - A1. ✅ **done (D35).** Docker-backed ephemeral-MariaDB test harness (`tests/support/docker_mariadb.rs`,
    feature `docker-tests`, skips cleanly with no daemon).
  - A2. ✅ **done (D35).** MariaDB live suite — verbatim codegen-lowered SQL through `serve::dispatch`
    against real `mariadb:11.4`, ran green.
  - A3. ✅ **done (D38).** Postgres driver + live suite (`src/postgres.rs`, `tests/postgres_integration.rs`),
    ran green against real `postgres:16`. All three dialects now clear DoD #1's real-server bar.
  - A4a. ✅ **done (D59).** Remaining DoD-#1 live coverage — **keyset + offset pagination** (full→full→
    short + tampered-cursor→400) and **soft-delete + restore read-back** proven against real
    MariaDB/Postgres (mirroring the SQLite live tests), so DoD #1 is symmetric across all three
    dialects. Surfaced + fixed a real Postgres bug: numeric binds now go in **text format** (a binary
    i64 mismatched the `int4` Postgres infers from the keyset guard's `:keyset_active = 0` literal).
  - A4. ✅ **done (D65). Live-DB hardening** — **statement timeouts** (per-dialect: MariaDB session
    `max_statement_time`, Postgres startup `statement_timeout`, SQLite `busy_timeout`), **deadlock-retry**
    (drivers classify 1213/1205 · 40P01/40001 · `SQLITE_BUSY` into `DbErrorKind::Deadlock`; the mutation
    path re-runs the whole transaction a bounded 5× with jittered backoff, then a 503), and
    **pool-exhaustion → fast 503** (bounded checkout wait — MariaDB `try_get_conn`, Postgres r2d2
    `connection_timeout` — surfaced as `DbErrorKind::PoolExhausted`, never a hang). Timeouts live on
    `PoolConfig` (`checkout_timeout` + `statement_timeout`). **Verified live:** a `SELECT SLEEP/pg_sleep`
    aborted at the timeout, two crossed-lock concurrent txns deadlock with exactly one aborted + one
    committed, and a pool-of-one fails the second checkout fast as PoolExhausted — all against live
    `mariadb:11.4` + `postgres:16` (Docker); the retry loop + SQLite busy classification are unit-proven.

**Track B — example projects (DoD #2). ✅ COMPLETE (B1/B2 SQLite D60, B2 MariaDB + Postgres D61); DX
rebuild D63.** One worked, runnable project per target DB, all three green via `cargo run`. **DX rebuild
done (D63):** the three quickstarts are now genuinely copyable references, not integration tests —
schema setup is `based migrate apply` (checked-in `migrations/`), the typed client is a checked-in
`src/client.rs` from `based gen client -o src/client.rs --embedded` (new CLI flag) consumed via
`client::embedded(&engine)` (zero bridge), seeding is the client's own `create_org`/`create_user`
mutations (zero raw SQL), and `DATABASE_URL` comes from a committed `.env` (dotenvy). No `build.rs`.
First **live Postgres `migrate apply`** landed here (worked unchanged). User flow: set `.env` →
`based migrate apply` → `cargo run`.
  - B1. ✅ **done (D60).** Scaffolded `examples/` as standalone crates **outside** the workspace (root
    `Cargo.toml` `exclude = ["examples"]`, so `cargo test --workspace` never builds them; each has its
    own `target/`, gitignored).
  - B2 (SQLite). ✅ **done (D60).** `examples/sqlite-quickstart` — a reduced-commerce `.bsl` schema
    (`@scope Tenant`, nested to-one shape, keyset page, soft-delete) consumed through the **generated
    typed client over the in-process `Engine`** against a live bundled-SQLite DB. `build.rs` regenerates
    the client + DDL from the schema each build (no checked-in generated code); `cargo run` executes and
    asserts the end-to-end scenario (create → read-your-writes in declared shape → get → list/scope →
    keyset paginate → soft-delete/restore round-trip) and exits 0 only if it passes.
  - B2 (MariaDB + Postgres). ✅ **done (D61).** `examples/mariadb-quickstart` + `examples/postgres-
    quickstart` — the *same* schema/client/bridge/scenario/assertions as the SQLite slice, differing only
    where the server forces it: the driver (`ShardRouter`/`MariaDb`, `PgRouter`/`PostgresDb` over a live
    `DATABASE_URL`, checked out as the engine's `Db`), the id generator (`UuidGen` — native `uuid` id
    columns reject non-uuid ids), and the fixture ids (real UUIDs). Each resets its tables on startup so
    it is re-runnable; ran green against live `mariadb:11.4` + `postgres:16` (Docker). The Postgres slice
    surfaced + fixed a real driver bug (binary-format result decode for uuid/timestamptz/date/jsonb, D61).

**Track C — VS Code extension (DoD #3, independent, may run in parallel).**
  - C1/C2. ✅ **done (D36).** Scaffolded `editors/vscode/` (TS + `vscode-languageclient`): `.bsl`
    registration, TextMate grammar, launches `based-lsp` over stdio, wires diagnostics/inlay/hover;
    `.vsix` packages.
  - C3. ✅ **done (D40).** Per-file manifest resolution — each open file resolves to its nearest
    `based.toml`, one snapshot per project, so embedded schemas resolve cross-file (no spurious E0110).
  - **C4. ✅ done (D68). Feature-parity audit + fill-in** (baseline editor features a `.bsl` author
    expects). *Framing (user, 2026-07-06): the LSP exists to power the editor tooling, not the reverse.*
    The audit checklist lives in `editors/vscode/README.md` ("LSP capability audit"). Delivered: document
    symbols (D44), completion (D45), go-to-def (D43), find-references (D52), rename + prepareRename (D53),
    workspace symbols `⌘T` (D54), and the D68 fill-in — **folding ranges + selection ranges** (off the
    parsed decl spans; capability-advertised, unit-tested over the commerce fixture, binary rebuilt).
    `language-configuration.json` verified (bracket/auto-close/`#`-comment — already complete). **Code
    actions declined:** `W0103` anchors on the query, not the model needing the `@index`, and carries no
    target span — a correct quick-fix needs new lint→diagnostic plumbing, not cheap (documented). Was
    out of scope: formatting, signature help, call hierarchy, semantic-tokens re-do, debugging. The
    **`based fmt` formatter** + `format-document` LSP directive are queued next.
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

**Track E — migration generation (DoD #5, independent, spec-first). ✅ COMPLETE (E1–E4 D37/D39/D41/D42,
E5 D67).** *Design settled 2026-07-06;
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
  - E5. ✅ **done (D67). `@was` rename directive** + offline drift diagnostic + `raw(dialect)` up step.
    Field/model `@was("old")` (parser → `Field.was`/model decorator → `RModel.was`/`RMember.was`; sema
    `E0190` no-op / `E0191` old-name-still-live). Renames are snapshot-authoritative: `Snapshot.renames`
    persisted in `schema.snap`, so `diff_snapshots` emits one `rename table`/`rename column` step (never
    an auto-guess; a spent `@was` is inert) rendered as a data-preserving `ALTER … RENAME` per dialect.
    `raw(dialect)` escape (`Step::Raw`) authored into `up.mig`, recovered by `parse_raw_steps`, layered
    onto structural steps for the matching target; `verify` reports a raw-carrying migration `partial`.
    Offline drift: LSP `W0108` ("N uncaptured changes — run `based migrate gen`", anchored per model) +
    spent-`@was` `W0107`; `based migrate verify` is the CLI twin. Proven live on `postgres:16` +
    `mariadb:11.4` (rename preserves data; raw backfill applies).

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

**Track D — deploy + keep-proven (DoD #4, last). ✅ COMPLETE (D1 D66; D2/D3 D64).**
  - D1. ✅ **done (D66).** `based serve` container image (`docker/Dockerfile` + `entrypoint.sh` +
    `healthcheck.sh` + `docker/README.md`). Multi-stage (rust builder → `debian:bookworm-slim`, ~122 MB,
    BuildKit cache mounts, unprivileged); carries no schema — the project mounts at `/app`, all else env
    (`BASED_DATABASE_URL`/`DATABASE_URL`, `BASED_LISTEN`, `BASED_MIGRATE_ON_START` opt-in migrate-then-serve);
    `HEALTHCHECK` probes `/healthz` (D26). Enabling seams: `based serve` is now **dialect-aware** (was
    MariaDB-only — branches on the manifest dialect to build MariaDB/Postgres/SQLite backend), and `--listen`
    reads `BASED_LISTEN` + the db-url resolver honors `DATABASE_URL`. CI: `make ci-image` builds + smoke-boots
    it against bundled SQLite (`ci/smoke-image.sh` → `/healthz`+`/readyz` 200), thin `image:` job in `ci.yml`.
    Verified live: image served `postgres:16` end-to-end (migrate-on-boot, scoped write/read, tenant
    isolation, `HEALTHCHECK` healthy, SIGTERM drain).
  - D2. ✅ **done (D64).** CI running the real-DB suites (A) + example builds (B) + extension build (C) +
    migration apply (E4). Substance lives in portable `make` targets (`ci-workspace`, `ci-live-mariadb`,
    `ci-live-postgres`, `ci-examples`, `ci-extension`; `dev-db-up`/`dev-db-down` for local DBs) invoked
    by plain `cargo`/`based`/`npm` — runnable on any CI or a laptop. `.github/workflows/ci.yml` is a
    **thin example wrapper**: five jobs provisioning `services:` `mariadb:11.4` + `postgres:16` (health
    checks) + Node, each calling a `make` target. All targets ran **green locally** against live servers.
  - D3. ✅ **done (D64), satisfies the CI-migrations ask.** The live suites now honor an external
    `TEST_MARIADB_URL`/`TEST_POSTGRES_URL` (connect to a CI service container instead of self-spinning;
    fall back to self-spun locally when unset) with a portable readiness-wait (`ci/wait-for-db.sh`
    `/dev/tcp` for the CLI path, an in-process poll for the suites) and per-test schema reset so a shared
    server is re-runnable. `based migrate apply` runs non-interactively against the provided DB in both
    the live MariaDB apply suite and all three example scenarios.

**Track F — source hygiene pass (quality, cross-cutting; standalone value, off the DoD critical
path — worked when it won't preempt A/B/D/E).**
  - F1. ✅ **done (D69). Comment hygiene swept across all source.** Every `crates/**/*.rs` checked for
    build-time / WIP narration (`sqlite.rs` first — it read clean, since D65 had already rewritten it
    well); the source already reads as finished (the standing rule in Conventions had kept it clean
    incrementally), so only three residual narration bits were tightened (`migrate.rs` `schema.snap`
    grammar header, `client.rs` `ClientTarget` "for now" + embedded-bridge historical phrasing). No live
    inline TODO/FIXME markers remained; the "deferred to the live-DB slice" scope notes are legitimate
    architecture docs (their deferred items already sit in the Deferred list). Comment-only; gate green
    (`test --workspace --all-features` + `fmt --check` + `clippy`). The standing rule is in Conventions.

## Track N — async-native pivot (owner decision 2026-07-10; TOP PRIORITY)

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
- **N2. Streaming reads (claims N1's payoff immediately).** The driver seam already streams —
  `fetch` returns a sqlx-backed row stream on all three dialects (D84 decision 3); N2 surfaces it.
  - ✅ **Spec/design slice (D85, `spec/syntax/streaming.md`).** Opt-in is the signature return form
    `-> stream Shape` (grammar extended; contract lives where the client surface is generated from);
    wire = NDJSON envelope-per-line with a mandatory terminal `done`/`error` line (no terminal line
    = truncation = transport error; pre-body failures keep real statuses); client = same-named
    method returning `Result<RowStream<Shape>, ClientError>` with per-item `Result`, drop = cancel;
    `page` forbidden (E0201), `get`/mutations can't stream (E0200/E0202), everything else (filters,
    sorts, shapes, scope, soft-delete, index lint) composes unchanged on the single read path.
  - Remaining implementation slices:
    - Parser/sema/fmt for `stream` ret form + E0200/E0201/E0202; LSP surfaces it; conformance goldens.
    - Runtime streaming dispatch: a public engine surface yielding shaped rows off the existing
      `fetch` stream + the axum NDJSON body (terminal-line framing, drain/cancel behavior).
    - Generated client: `Transport` streaming call + HTTP NDJSON parsing + embedded bridge over the
      engine's row stream; OpenAPI emitter for the NDJSON response.
    - Acceptance gates: mid-stream DB error observed as the in-band `error` line live; truncation →
      transport error; drop-mid-stream releases the connection (extends the I2 cancel gate).
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

## Track T — core DB feature parity (owner-approved 2026-07-09; PAUSED behind Track N)

A confirmed 6-item queue closing the gap to a general DB-first DSL (commerce is only a *named*
example, not the domain). **Paused at T3 behind Track N — N1 recolors the execution paths T3–T6 build
on (recolor first, then resume T3 on the async core).** Worked in order; each iteration marks its
item + D#. Tier-2 follow-ons below.

- **T1. ✅ done (D82). Enum type — string + numeric kinds.** `enum Name { … }` first-class scalar; a field
  typed by an enum name is a stored column (not a relation), referenced in value position by variant **name**
  (`where status = paid`). Kind is inferred from the variant values: a **string enum** (bare or explicit
  `paid = "PAID"`, name≠value ok) stores text + `CHECK (col IN ('…'))` and allows `= != in`; an **int enum**
  (`low = 0, …`) stores an integer column + `CHECK (col IN (0,…))` and additionally allows ordered
  `< > <= >=`. Uniform column+CHECK (migration-simple vs. native enums); client emits a real Rust enum
  (string → serde-rename; int → explicit discriminants + a hand-rolled i64 serde, no new dep); OpenAPI a
  string- or integer-enum; snapshot encodes kind+values (`enum(…)` / `enum:int(…)`) so a variant/kind change
  diffs. Editor: variant go-to-def/find-refs/rename (enum-local) + hover. Codes E0104/E0106/E0154/E0155/
  E0156/E0157/E0158. Commerce keeps the string `Order.status`; int enums + name≠value live in tests. Proven
  live SQLite (create→wire-value shape, string + ordered-int filter, CHECK rejection) + MariaDB/Postgres suites.
- **T2. ✅ done (D83). decimal + float** — exact `decimal(p, s)` (money; bare `decimal` = `decimal(38, 9)`)
  + 64-bit `float`. New `Primitive::Float`/`Primitive::Decimal { precision, scale }` thread through `sql_type`
  (now `String`; `DECIMAL(p,s)`/`NUMERIC(p,s)`/`DOUBLE`/`REAL`), the numeric `prim_family` (int/float/decimal
  inter-compare), and the neutral snapshot (`decimal(p,s)`/`float`, so a precision change diffs). Decimal is a
  **JSON string** on the wire (lossless, never an f64) → client `rust_decimal::Decimal` via the `serde-str`
  feature; float → `f64`/JSON number; openapi string-decimal / number-double. Exact defaults: `Literal::Float`
  replaced by **`Literal::Decimal(String)`** (exact source text). Runtime carries a decimal as its wire string
  end-to-end — `rust_decimal` stays OUT of based-runtime; the only decode change is a **`pg_numeric`** binary
  decoder (Postgres sends `numeric` in binary) + SQLite `TEXT` storage (exact; comparison lexicographic there).
  New code **E0159** (bad precision/scale or bad decimal default). Commerce `Order.total` → `decimal(12, 2)`;
  all three quickstarts converted + re-run green live. Detail: D83.
- **T3. atomic update expressions** — `update … { total = total + $n }` (self-referential SET), lowered to
  a real SQL expression, not a re-read-then-write.
- **T4. aggregations + group-by + having** — `count`/`sum`/`avg`/`min`/`max`, `group by`, `having`.
- **T5. m2m + upsert** — many-to-many join tables + `create … on conflict update` (upsert).
- **T6. referential actions** — FK `on delete`/`on update` cascade/restrict/set-null (opt-in, visible).

Tier-2 (later): for-update locking, computed shape fields, `distinct`, time/bytes types.

## Post-completion backlog (surfaced after the DoD was met)

The DoD is fully met; these are items raised *after* completion — the standing **polish / UX /
bugfinding** track. They are not deferred-and-forgotten (§Deferred) — they carry real standalone value
a user would notice, but sit outside the acceptance criteria the project was built to. Worked when
picked up, hardest-value-first within the section. H2–H4 are editor/LSP work (the extension is
feature-complete per DoD #3 but has rough edges); H5 is cross-cutting.

- **H1. ✅ done (D70). Typed ids in the generated client.** `based gen client` emits per-entity phantom
  newtypes `Id<entity::M>` (`#[serde(transparent)]`, wire unchanged; `pub mod entity` of tag types) for a
  model's own `id`, a relation param/FK, and a `$ctx` relation — so `Id<User>` ≠ `Id<Org>` and an
  id-transposition is a compile error, not a runtime FK failure. No blanket `From<String>` (raw is the
  greppable `Id::from_raw`); create→use chains need no conversion (a `create_*` result already IS the typed
  id). Param entities come from the edges sema already resolves (query binding / mutation-body write), so no
  new analysis. `openapi` needed no change (transparent → still a `uuid` string). Spec: calling.md "Typed ids"
  + D70. Landed across `based-codegen::client` + all three `examples/*/src/{client.rs,main.rs}`; three
  quickstarts ran green live.
- **H1a. ✅ done (D73). Typed keyset cursor in the generated client.** Mirrors H1/D70 for pagination: the
  bare `Option<String>` cursor (the `Page<T>.cursor` field + the keyset next-page input) is now an opaque
  `Cursor` newtype — a single `#[serde(transparent)]` type (wire unchanged), not generic-per-query (a
  cursor is checksum/arity-validated in `cursor.rs`, not entity-typed like an id). A page hands one back
  and the caller feeds it straight to the next call (no conversion); raw construction is the greppable
  `Cursor::from_raw`, no blanket `From<String>`. Spec: calling.md pagination note + D73. Regenerated all
  three `examples/*/src/client.rs` (their `main.rs` already threads `p.cursor` back untouched); three
  quickstarts ran green live exercising pagination.
- **H7. Production-grade error handling (user-raised 2026-07-08, high-priority).** A pass to make the
  errors a *library user* meets structurally sound + ergonomic (`Display`, `std::error::Error`, stable
  codes, clear messages), driver limits notwithstanding. **Audit found:** (a) the generated client's
  `ClientError` was `struct ClientError(pub String)` — no `Display`/`Error`/`source()`, no code/status,
  and the embedded bridge flattened the wire `{code,message,status}` to an opaque string a caller could
  only string-match; (b) runtime `PlanError`/`DbError`/`RunError` were `Debug`-only (no `Display`/`Error`/
  `?`-chaining), and the stable wire codes lived only as string literals in `serve` (drift risk);
  (c) CLI (`based migrate`/`gen`/`serve`) surfaces `anyhow`-blob errors; (d) example `main.rs` uses
  `.expect` throughout. **✅ slice done (D71): the client + runtime errors it surfaces.** `ClientError`
  is now a `kind`(`Transport`/`Decode`/`Api{status,code}`)-carrying `Error` with `Display` + `source()`
  (Arc-backed, stays `Clone`) + `code()`/`status()`/`message()` accessors + `transport`/`decode`/`api`
  constructors; the embedded bridge preserves the server's status+code+message. `PlanError`/`DbError`/
  `RunError` implement `Error`+`Display` with a single-source `code()` that `serve` consumes (maps only
  status); `DbError::code()` distinguishes `deadlock`/`pool_exhausted`. Regenerated all three
  `examples/*/src/client.rs`; ran green live on all three dialects. **✅ CLI slice done (D72):**
  `based-cli` gained a structured `CliError` (`Display` + `source()` chaining + a `usage`/`failure`
  exit-code convention — 2 for a config/usage mistake matching clap, 1 for an operational failure) that
  reuses D71's typed `MigrateError`/`DbError` as the cause instead of re-flattening them to strings;
  `anyhow` dropped from the crate; the rustc-style parse/sema diagnostic rendering is untouched (those
  paths return a summary-only error). **✅ edge slice done (D75):** the `http` listener's own pre-dispatch
  failures now share a `code()`/`status()`-keyed `EdgeError` registry (`BadContext`/`BadBody`/`Draining`/
  `NotReady` — one source of truth, `From<EdgeError> for WireResponse`) instead of scattered string
  literals, mirroring `PlanError`/`DbError`; the pool-checkout path now reuses the driver's own classified
  `DbError::code()` (so a `pool_exhausted` checkout no longer masquerades as `database_error`). **✅
  reference slice done (D76):** `examples/sqlite-quickstart/src/main.rs` is now an idiomatic `?`-based
  `Result` flow (`main -> Result<(), Box<dyn Error>>`, every client call threads `?`, helpers return
  `Result<_, ClientError>`) with a **step 6** that matches a deliberately malformed cursor's `ClientError`
  on `kind()` — asserting the `Api { status: 400, code: "bad_cursor" }` class and reading `code()`/
  `status()` — so it teaches handling the typed surface rather than unwrapping; scenario invariants stay
  asserts (a demo doubles as a smoke test); ran green via `based migrate apply` → `cargo run` (exit 0).
  (Sub-item (iii), a structured migration error type, is subsumed — `based migrate apply` now surfaces
  the typed `MigrateError` through the `CliError` chain, no longer re-stringified.) **Follow-up:** the
  MariaDB/Postgres quickstarts still use `.expect(...)`; the identical pattern transfers verbatim but
  needs a live server to re-verify. **H7 is now complete** across its four sub-items (client/runtime D71,
  CLI D72, edge D75, example D76).
- **H2. ✅ done (D78). `based fmt` formatter + `formatting` LSP directive.** New front-end crate
  `based-fmt` (`format_source(&str) -> Result<String, Vec<Diagnostic>>`): parse → AST pretty-print, a
  pure function of the AST except comments (lexer-discarded), which it recovers from source and re-emits
  in slot (every `.bsl` comment is a column-0 line, before a decl or between a model's decorators, never
  inside a body — a line-range lookup). Deterministic + idempotent + reparse-stable; converges to the
  worked examples' style (byte-exact no-op across all committed schemas): type-column + inverse-ref
  alignment, width-gated inline vs multi-line shapes, clause-count query-block layout, minimal-paren
  predicates. `based fmt [--check]` (manifest/glob discovery, in-place write or nonzero `--check`,
  `CliError` not anyhow); LSP advertises `formatting` + returns a full-document `TextEdit` via a thin
  `Snapshot::format_document` over the same `based_fmt` — one printer, no CLI/editor divergence. The
  C4/D68 "revisit with `based fmt`" note is now closed.
- **H3. ✅ done (D80). Comprehensive rename symbol (user-raised 2026-07-08).** Extended D53's rename to
  every renameable symbol the reference index missed: (a) **params** — a callable's `min: int` decl ↔ its
  `$min` body uses, callable-local (a sibling callable's same-named param is untouched); (b) **`$ctx`-field
  rename** — one bag field renames its `scope … = $ctx.field` binding and every callable use (the scope
  *column* + same-named model columns are deliberately left alone — a polymorphic contract name shared by
  every scoped model, excluded like filter roots); (c) **callable name** — a wire endpoint with no
  in-`.bsl` refs, so the declaration alone rewrites; (d) **`@was`-aware physical rename** — renaming a
  field/model mapped to a *live* DB column/table (in the latest `migrations/**/schema.snap`) also inserts
  `@was("old")` so the next `based migrate gen` renames-preserving-data instead of drop+add (skipped for a
  `(column …)`/`@table` override, an existing `@was`, an inverse member, or no captured snapshot). Rename
  spans the whole owning project (cross-file), not across independent manifest projects. All in
  `based-lsp/src/compile.rs` (the D53 server handler is unchanged); unit-tested per case with the applied
  `@was` reparsed to confirm it round-trips. Full gate green (incl. live MariaDB/Postgres).
- **H4. ✅ done (D74 + D77). Editor-surface conciseness (user-raised 2026-07-08).** Editor gravy now
  states *what a symbol is*, never *how to use the system*. **Positive-framing half (D74):** the `Page`
  hover / doc-string reads positively (rows + an opaque cursor), and the define-by-negation pattern was
  swept from source + userland (genuine behavioral guarantees kept). **Conciseness half (D77):** the
  scope hover (`based-facts::scope_detail`) is trimmed to `` scope `Tenant`: filter `…`; governs … ``
  (name + predicate + governed models; the confinement/opt-in prose moved to auth.md), the `$ctx` hover
  (`ctx_fact.detail`) to `` request context: this query requires `$ctx` [bag] `` (the concrete bag; the
  wire-contract sentence dropped), and the duplicate `requires […]` **inlay** is removed —
  `FactKind::CtxRequirement` now carries no inlay (the hover holds the bag), so there is no
  hover↔inlay redundancy. Facts tests updated to the concise strings.
- **H5. Doc + comment critical-eye pass, project-wide (user-raised 2026-07-08).** Two rules, enforced
  everywhere a user reads: (a) **no `D#`/decision-refs in any userland surface** — editor hover/inlay
  strings, CLI output, `examples/**` (comments + READMEs), `docker/README.md`, generated-code headers.
  A user must never parse `D50`. The D50 scrub covered *facts editor strings*; this widens it to every
  user-facing surface and re-verifies. (b) **comments say what + why-if-needed, never how; no design
  rationale inline** — rationale lives in spec/decisions/PLAN; the only inline exception is a genuinely
  blocking TODO, which also gets a PLAN line. F1/D69 swept `crates/**` for WIP narration; this is the
  follow-on for *wordiness + rationale-in-comments + userland `D#`*, and explicitly includes
  `examples/**` and the `spec/examples` comments a user meets first. Reinforces the Conventions rule.
  **Substantially advanced (D74):** every userland surface is now `D#`-free — the emitted SQL re-select
  comment, two OpenAPI `description` strings, the `--embedded`/`openapi` `--help` doc-comments, an
  `E0181` sema **diagnostic message**, the three example `main.rs`/`README.md`, `docker/*`, and the
  regenerated `examples/*/src/client.rs`; and `crates/based-codegen/src/**` is fully `D#`-free with its
  overlong module/block comments compressed to what+why (the user flagged that crate by name). **Left
  standing on purpose:** internal `///` doc-comment `D#` refs in the *other* crates (`based-sema`/
  `runtime`/`ast`/`parser`/…) — the standing rule permits `D#` in internal doc comments (they aid the
  reviewer, are not a userland surface). A project-wide *wordiness* pass beyond `based-codegen` remains.
- **H6. Adversarial correctness sweep (user-raised 2026-07-08).** A standing bugfinding pass over the
  built surface — codegen SQL edge cases, runtime binding/nesting, scope/soft-delete predicate
  composition — driven against a live DB, not just unit tests. Scope each sweep to one subsystem; file
  what it finds as its own item. Open-ended by nature; run when the higher-value H-items are quiet.
- **H9. ✅ done (D81). `@scope` now confines a nest-only scoped child (correctness/security, from H6).**
  A scoped model reached **only** through a nested shape sub-object (`field { … }` to-one/to-many or
  `field -> Shape`, D79) was not confined by its `@scope` though soft-delete was — a cross-scope read
  leak, from a stale D34 invariant (the shape walks skipped `Nest`/`NestRef`, so a nest-only scoped child
  never entered `scope_inject`/`ctx_requires`). Fix: **both** shape walks (`walk_shape_join` in `scope.rs`
  + `walk_shape_scope` in `ctx.rs`, kept byte-for-byte parallel) now recurse into `Nest`/`NestRef` in the
  child model context (NestRef body via `cx.shape_bodies`, D79 cycle guard). Compile-time is the primary
  guarantee — nesting into a scoped model **counts as touching it**, so an unsatisfied `scoped …` set is
  `E0185`, not a runtime leak; the existing runtime injection (to-one join `ON`, to-many subquery `WHERE`)
  is defense-in-depth. **Type optionality mirrors the schema only** — scope never widens a nested field to
  `Option` (client derives it from the relation's own nullability; a genuine cross-scope FK surfaces as a
  decode error, not a softened type). Verified: sema (nest-only child on a divergent axis → E0185; both
  axes → clean), codegen SQL (predicate in the to-one `ON` + to-many `WHERE`) + type-optionality unit test,
  and live on SQLite (divergent-axis nested read excludes the out-of-scope to-many item + NULLs the
  out-of-scope *optional* to-one). No commerce/`examples/**` fallout (their only scoped model nests into
  the unscoped `User`).
- **H8. ✅ done. embed.rs generated-client mirror de-staled + regen-gated (user-raised 2026-07-09).**
  `crates/based-runtime/tests/embed.rs` inlined a hand-copied `mod client` claiming to be verbatim
  `based gen client --embedded` output "verified by the regen gate" — but no gate existed, and it had
  drifted (predated D71: missing `ClientError::{transport,kind,status}`, the per-kind `Display`, the
  `source()` impl; trimmed `Cursor`/`ClientErrorKind` docs). Fix: the verbatim output now lives in
  `tests/support/embedded_client.rs` (a subdir, so it's neither an auto test target nor rustfmt-followed
  through `include!`), `mod client` just `include!`s it, and a new `generated_client_is_current` test
  regenerates from `SCHEMA` via `based_codegen::client::client_with` and asserts byte-equality — the
  real gate, so the mirror can never silently rot again.

## Pipeline (data flow)

```
*.bsl ──manifest::discover──▶ files
      ──parser::parse_file──▶ [Decl]           (per file; recovers at decl boundary)
      ──fmt::format_source──▶ canonical `.bsl` text  (based fmt [--check] + LSP formatting ✅ D78)
      ──sema::check─────────▶ CheckedSchema + [Diagnostic]
      ──codegen::sql::ddl───▶ SQL DDL          (M2 ✅; dialect-aware: MariaDB + SQLite + Postgres, D28/D29)
      ──codegen::sql::dml───▶ query SELECTs    (M3 read side ✅)
      ──codegen::sql::mutations─▶ INSERT/UPDATE/DELETE  (M3 write side ✅)
      ──codegen::client─────▶ typed Rust client (M4 ✅)
      ──codegen::openapi────▶ OpenAPI 3.1 doc → polyglot clients (D24 ✅)
      ──codegen::migrate────▶ schema.snap + up.mig + per-dialect migration SQL (E2/E3 ✅ D39/D41)
      ──facts───────────────▶ engine-derived facts    (M5 ✅)
                              └─ based-lsp ──▶ editor inlay hints + hover + diagnostics + go-to-def + find-refs + rename + symbols + completion + folding + selection ranges
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
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. `Decl::Enum` (variants carry an optional `= STRING\|INT` value) + `DefaultVal::Variant` (D82); `Primitive::Float` + `Primitive::Decimal { precision, scale }`, and `Literal::Decimal(String)` (a fractional literal's exact source text; replaced `Literal::Float`) so a decimal default/value is byte-exact (D83). |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). `$ctx` is inferred in sema, not declared here (D4). |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. Parses `decimal(p, s)` (optional args → `decimal_args`, bare = `(38, 9)`) + `float`; a `FLOAT` token → `Literal::Decimal` exact text (D83). |
| based-fmt | ✅ works | canonical `.bsl` formatter (D78): `format_source` = parse → AST pretty-print, a pure function of the AST except lexer-discarded comments (recovered from source, re-emitted in slot). Deterministic + idempotent + reparse-stable; byte-exact no-op over every committed schema (type-column/inverse-ref alignment, width-gated inline shapes, clause-count query blocks, minimal-paren predicates). Shared by `based fmt` + the LSP `formatting` handler. |
| based-sema | ✅ stable | resolution + checks + lints + `CheckedSchema` IR (incl. `@was` rename directive on `RModel`/`RMember` + `E0190`/`E0191`, D67; enum decls → `REnum` with inferred `kind` (string/int) + valued variants, kind/dup/ordered-op/membership checks E0104/E0106/E0154/E0155/E0156/E0157/E0158, D82); decimal/float numeric family + `decimal(p,s)` range / decimal-default validity (E0159, D83). Detailed behaviour in the next section. |
| based-cli | ✅ works | `based check`; `based fmt [--check]` (canonical formatter over the manifest glob, in-place or check-only, D78); `based gen sql\|client\|openapi`; `based facts [--json]`; `based migrate gen\|render\|apply\|status\|verify`; `based serve`. Structured top-level `CliError` (`Display` + `source()` chaining; exit 2 usage/config, exit 1 operational failure) reusing the runtime's typed `MigrateError`/`DbError` as the cause; rustc-style parse/sema diagnostics via `render.rs` (D72). |
| based-codegen | ✅ stable | `sql::ddl\|dml\|mutations` → dialect-aware DDL/SELECT/INSERT-UPDATE-DELETE (MariaDB/SQLite/Postgres, D28/D29) through one `Dialect` quoting/type seam; declared-shape re-select on every surviving write (create-keyed D12 + update/delete/restore where-keyed D58); nested to-one shape sub-objects (D55) + to-many nested arrays via correlated-subquery JSON aggregation incl. self-ref aliasing (D57), each with a nest-reached scoped child's `@scope` injected into the join `ON` / subquery `WHERE` (D81) + keyset-cursor pagination (lexicographic `WHERE` + hidden `__keyset_` columns, D56); `client` → typed Rust client (nested `Vec<…>` for to-many, paginated inputs carry a typed `cursor`/`offset`, D56/D57; **per-entity phantom-typed ids** `Id<entity::M>` — transparent wire, `from_raw` escape, no blanket `From<String>`, D70; an opaque **`Cursor`** newtype for the keyset surface (single `#[serde(transparent)]` type, `from_raw` escape, D73); a structurally-sound **`ClientError`** — a `kind`(Transport/Decode/Api{status,code})-carrying `std::error::Error` with `Display`+`source()` (Arc-backed, stays `Clone`) + `code()`/`status()`/`message()` accessors, the embedded bridge preserving the wire status+code+message, D71) with an **opt-in in-process embedded bridge** (`ClientOptions::embedded` / `client_with` → emits `client::embedded(&engine)` over `based_runtime::Engine`, so an embedder writes zero `Transport` plumbing; referenced by path, no based-runtime dep; D62); `openapi` → OpenAPI 3.1 (D24; an enum field → a string- or integer-schema with `enum: [...]`, D82); enum columns →
a text-or-integer column + named `CHECK (col IN …)` in DDL (string or numeric kind, D82), a real Rust enum in the
client (string → serde-rename incl. name≠value; int → discriminants + hand-rolled i64 serde, no new dep, D82),
and `enum(…)`/`enum:int(…)` in the neutral snapshot so a variant or kind change diffs (D82); decimal/float scalars → per-dialect `DECIMAL(p,s)`/`NUMERIC(p,s)`/`TEXT`(SQLite) + `DOUBLE`/`REAL` (`sql_type` now returns `String`), the client emits `rust_decimal::Decimal` (JSON-string wire via `serde-str`) + `f64`, openapi string-decimal/number-double, and `decimal(p,s)`/`float` in the neutral snapshot (D83); `migrate` → `schema.snap`/`up.mig` diff (D39) + `render_sql` per-dialect migration SQL (D41) + `sql_statements`/`content_hash` for apply (D42) + scope serialization (D50) + `@was` snapshot-authoritative renames (`Snapshot.renames` persisted → `rename table`/`rename column` steps → per-dialect `ALTER … RENAME`), the `raw(dialect)` escape step (`parse_raw_steps`), and the offline `drift` helper (D67). |
| based-facts | ✅ stable | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>` — the "show, don't write" facts (inferred inverses, join-key indexes, per-callable `$ctx` bags, resolved query shapes, scope contract), span-anchored, editor-string-scrubbed of internal refs (D50). |
| based-lsp | ✅ works (C4 complete) | tower-lsp server; recompiles on edit (unsaved buffers overlaid on disk), publishes diagnostics + inlay + hover + go-to-def (D43) + document symbols (D44) + completion (D45); per-file manifest resolution (D40); scope go-to-def/hover (D50); field-reference go-to-def + broad declaration hover + command-clickable inverse inlay (D51); find-references incl. filter calls + inverse back-edge, filter go-to-def (D52); rename + prepareRename reusing the reference index, back-edge excluded (D53), extended to params, `$ctx` bag fields, callable names, and `@was`-aware physical rename of a live column/table (D80); workspace symbols (⌘T) across every open project, fuzzy-filtered (D54); offline migration-drift diagnostic `W0108` + spent-`@was` `W0107` (diffs the latest `schema.snap` against the schema, no DB, D67); folding ranges (per multi-line decl body) + selection ranges (token→field→decl→file), both off the parsed decl spans (D68); whole-document `formatting` returning one full-document `TextEdit`, delegated to a thin `Snapshot::format_document` over `based_fmt` (D78); enum-variant navigation — go-to-def/find-refs/rename (enum-local) + hover in value/default position, plus enum type-ref go-to-def/hover + document/workspace symbols for the enum + its variants (D82). |
| based-runtime | ✅ works (M6) | in-process engine (D18): `Compiled::load` reuses the front end + codegen lowering; `plan_query`/`plan_mutation` validate + bind (`?`/`$n` per dialect), `run_*` shapes rows / runs writes under one tx with declared-shape re-select on every surviving write (create-keyed D12 + update/soft-delete/restore where-keyed D58, read-your-writes); `nest_row` reassembles to-one sub-objects (dotted alias) + parses to-many JSON-array columns (`field[]`) into sub-object arrays (D55/D57); keyset pagination decodes the incoming `cursor` → `:keyset_` binds + mints the next opaque, checksum-validated cursor (`cursor`, D56). `serve::dispatch` is the wire core (maps `PlanError`/`DbError` to status; the machine `code` + message come from the errors' own `code()`/`Display`, one source of truth, D71); `http` the `based serve` listener (D21) with health/readiness/drain (D26); `embed` the socket-free door (D22); `idempotency` keyed write dedupe + fingerprint (D25/D31). Concrete drivers: `sqlite` (D27), `driver::MariaDb` + `ShardRouter` (D20/D35), `postgres` + `PgRouter` (D38; numeric binds are text-format so an i64 never mismatches an inferred `int4`, D59; result columns are read in binary format — uuid/timestamptz/date/jsonb decoded to their canonical strings, D61; a `numeric`/`decimal` column decoded from its binary base-10000 form to an exact string via `pg_numeric`, D83). A decimal rides the runtime as its exact wire string end to end (`Family::Text`; `rust_decimal` stays out of based-runtime) — SQLite stores it as `TEXT`, MariaDB returns it as text, Postgres via `pg_numeric` (D83). Keyset/offset pagination + soft-delete/restore proven live on all three dialects (D59). Live-DB hardening (D65): per-dialect statement timeouts + bounded checkout wait on `PoolConfig`, drivers classify deadlock/serialization codes into `DbErrorKind::Deadlock` (mutation path retries the tx a bounded 5× with backoff) and pool saturation into `DbErrorKind::PoolExhausted` (fast 503), proven live on MariaDB/Postgres. `migrate` = live apply + ledger (D42). `based serve` is dialect-aware — the CLI branches on the manifest dialect to build the MariaDB/Postgres/SQLite backend (D66). Packaged as a container image (`docker/Dockerfile`, D66). *Open:* durable multi-instance idempotency store. |

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
- Named-shape nest references (shapes.md, D79): `field -> Shape` requires a declared shape (`E0132`)
  whose `from` equals the relation target (`E0133`); a shape transitively nesting itself by reference
  is `E0134` (cycle-guarded like D14 filter expansion).
- **Enums** (enums.md, D82): an `enum Name { … }` resolves to `REnum` (with an inferred `kind` — string vs
  int — and valued variants) in `CheckedSchema.enums`; a field typed by an enum name classifies as a scalar
  column (`MemberKind::Scalar` + `enum_name`; `ty` = text for a string enum, int for an int enum), not a
  relation. Kind inference rejects a mixed enum (`E0156`) and a duplicate wire value (`E0157`); a duplicate
  variant name is `E0104`. A `where`/`create`/`update` variant is a bare single-segment path checked for
  membership by **name** (`E0154`); an ordered comparison (`< > <= >=`) is allowed on an int enum but is
  `E0158` on a string enum. A `default <variant>` is checked (`E0155`, also catches a bare default on a
  non-enum column); an enum name colliding with a model/shape/scope/enum is `E0106`.
- **Decimal / float** (models.md, D83): `int`/`float`/`decimal` share the numeric operand family (a numeric
  literal binds to any; ordered ops allowed). A `decimal(p, s)` is range-checked (`1 ≤ s ≤ p ≤ 38` → `E0159`),
  and a decimal column's `default` must be a decimal literal (integer or fractional) — else `E0159`. Bare
  `decimal` is `decimal(38, 9)`.
- Implicit `id: Id` (D2); a model that declares its own `id` keeps it.
- Decorators: `@soft_delete` (covered-subset type check → `SoftMode`), `@created`/`@updated`
  (timestamp role), `@sort` (paths), `@table` (name override), `@was` (rename directive, below),
  unknown `@foo` → `W0101`.
- **`@was` rename directive** (migrations.md / D67): field-level `@was("old_col")` (modifier position)
  and model-level `@was("old_table")` (decorator) name a *previous* physical name for the migration
  diff. Sema catches the two locally-decidable mistakes: `E0190` (no-op self-rename) and `E0191` (old
  name is still a live column/table, so it can't be the rename source). The rename itself is
  snapshot-authoritative (codegen); the offline drift + spent-`@was` lints (`W0108`/`W0107`) are the LSP.
- **Named scope** (auth.md, D48/D49): a `scope Name (col: Type = $ctx.field)` decl (predicate = the
  restricted `col = $ctx.field` conjunction, checked at the decl site → `E0180`); `@scope Name`
  (repeatable → a DNF of alternatives) on the model; `scoped Name` / `unscoped("reason")` on the
  callable. Errors: `E0181` (create assigns a scope col), `E0182` (scoped callable declares neither
  scoped nor unscoped), `E0183` (unknown scope), `E0184` (model lacks the scope's column at a
  conforming type), `E0185` (scoped set ⊉ any alternative of a touched scoped model — *touched* includes
  a scoped child reached only through a nested shape sub-object, `field { … }` / `field -> Shape`, D81),
  `E0186` (a `create` can't auto-set a full alternative); `W0106` (stale unscoped). Scope injected into the
  root/write-target `WHERE` *and* every joined scoped model's `ON` (D34) — including a nest's to-one join
  `ON` and to-many correlated-subquery `WHERE` (D81); shard key bound to the scope `$ctx` field (D33).
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

Tests: `crates/based-sema/tests/check.rs` (~115 cases, positive + negative, keyed on diagnostic
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
- **Comments only where a reader can't infer *what* the code does from the code itself.**
  Most code needs none; a few key entry points get a short doc block. No cross-refs to
  D#/M#/PLAN/DoD/decisions in source. No WIP/rationale/narration — it reads as unfinished and
  leads humans *and* agents off task; that lives in spec/decisions/PLAN. TODOs go in PLAN.md /
  roadmap `.md`, not inline, unless genuinely must-do/blocking.
- **Keep this file lean.** PLAN.md is the resume read; shipped-work narration goes to
  `PLAN-archive.md`, per-decision detail to `spec/decisions.md`. Add a one-line status + D# here,
  not a paragraph.
- `spec/principles.md` are the tiebreakers, in order. `spec/decisions.md` (with its topic index)
  resolves anything the prose left open.
