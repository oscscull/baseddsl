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
is fully done per the Definition of Done** — every DoD item met, no open features, no open hygiene —
only deferred nice-to-haves remain. Batch-by-batch history is in `PLAN-archive.md`.

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
  paths return a summary-only error). **Remaining deferred H7 sub-items:** (ii) **HTTP listener edge
  errors** — the `http` edge's own pre-dispatch failures (bad body, bad `$ctx` header) are coherent but
  could share the code registry; (iv) **example `main.rs` as an error-handling reference** — convert the
  scenario to a `?`-based `Result` flow that demonstrates matching on `ClientError::kind()`/`code()`.
  (Sub-item (iii), a structured migration error type, is subsumed — `based migrate apply` now surfaces
  the typed `MigrateError` through the `CliError` chain, no longer re-stringified.)
- **H2. `based fmt` formatter + `format-document` LSP directive.** The canonical `.bsl` formatter; the
  editor's `format-document` handler delegates to it. Queued off C4/D68 (§Track C notes) — the one
  baseline editor feature deliberately left out of the C4 parity fill-in. Standalone user-visible value.
- **H3. Comprehensive rename symbol (user-raised 2026-07-08).** Rename works (D53) but partially. Today
  `references_at`/`rename_edits` (`based-lsp/src/compile.rs`) cover model/shape type refs,
  `@scope`/`scoped` refs, filter calls, field-path segments, and explicit inverse pairings (back-edge
  listed by find-refs, not rewritten). **Gaps to audit + close:** (a) **param rename** — a callable's
  `buyer: Id` decl ↔ its `$buyer` uses in the body; (b) **scope-column / `$ctx`-field rename**
  (`scope Tenant (org: …)` ↔ `$ctx.org`); (c) **callable (query/mutation) name rename**; (d) highest
  value — **`@was`-aware physical rename**: renaming a field/model that maps to a live DB column should
  offer to insert `@was("old")` so the generated migration *preserves data* (rename) instead of
  drop+add — ties rename to Track E. Confirm rename spans the whole workspace, not just the cursor's
  project. Touches `based-lsp` (reference index) + parser modifier position for `@was` insertion.
- **H4. Editor-surface conciseness (user-raised 2026-07-08).** Hover/inlay text *tutorializes* instead
  of naming things. `based-facts::scope_detail` teaches usage ("Every read and write … is confined … a
  callable opts in with `scoped` or out with `unscoped`") and `ctx_fact.detail` explains the client
  wire contract — both are spec material, not hover material. The `requires [org: -> Org]` **inlay**
  duplicates the ctx hover; if the hover carries the full contract, the inlay is redundant noise.
  **Rule:** editor gravy states *what a symbol is* when it isn't obvious, never *how to use the system*.
  Trim scope/ctx hovers to a one-line identity + the concrete filter/bag; drop or minimize the ctx
  inlay. Touches `based-facts::{scope_detail,ctx_fact}` + LSP inlay wiring; update the facts tests that
  assert the old strings.
- **H5. Doc + comment critical-eye pass, project-wide (user-raised 2026-07-08).** Two rules, enforced
  everywhere a user reads: (a) **no `D#`/decision-refs in any userland surface** — editor hover/inlay
  strings, CLI output, `examples/**` (comments + READMEs), `docker/README.md`, generated-code headers.
  A user must never parse `D50`. The D50 scrub covered *facts editor strings*; this widens it to every
  user-facing surface and re-verifies. (b) **comments say what + why-if-needed, never how; no design
  rationale inline** — rationale lives in spec/decisions/PLAN; the only inline exception is a genuinely
  blocking TODO, which also gets a PLAN line. F1/D69 swept `crates/**` for WIP narration; this is the
  follow-on for *wordiness + rationale-in-comments + userland `D#`*, and explicitly includes
  `examples/**` and the `spec/examples` comments a user meets first. Reinforces the Conventions rule.
- **H6. Adversarial correctness sweep (user-raised 2026-07-08).** A standing bugfinding pass over the
  built surface — codegen SQL edge cases, runtime binding/nesting, scope/soft-delete predicate
  composition — driven against a live DB, not just unit tests. Scope each sweep to one subsystem; file
  what it finds as its own item. Open-ended by nature; run when the higher-value H-items are quiet.

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
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). `$ctx` is inferred in sema, not declared here (D4). |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. |
| based-sema | ✅ stable | resolution + checks + lints + `CheckedSchema` IR (incl. `@was` rename directive on `RModel`/`RMember` + `E0190`/`E0191`, D67). Detailed behaviour in the next section. |
| based-cli | ✅ works | `based check`; `based gen sql\|client\|openapi`; `based facts [--json]`; `based migrate gen\|render\|apply\|status\|verify`; `based serve`. Structured top-level `CliError` (`Display` + `source()` chaining; exit 2 usage/config, exit 1 operational failure) reusing the runtime's typed `MigrateError`/`DbError` as the cause; rustc-style parse/sema diagnostics via `render.rs` (D72). |
| based-codegen | ✅ stable | `sql::ddl\|dml\|mutations` → dialect-aware DDL/SELECT/INSERT-UPDATE-DELETE (MariaDB/SQLite/Postgres, D28/D29) through one `Dialect` quoting/type seam; declared-shape re-select on every surviving write (create-keyed D12 + update/delete/restore where-keyed D58); nested to-one shape sub-objects (D55) + to-many nested arrays via correlated-subquery JSON aggregation incl. self-ref aliasing (D57) + keyset-cursor pagination (lexicographic `WHERE` + hidden `__keyset_` columns, D56); `client` → typed Rust client (nested `Vec<…>` for to-many, paginated inputs carry a typed `cursor`/`offset`, D56/D57; **per-entity phantom-typed ids** `Id<entity::M>` — transparent wire, `from_raw` escape, no blanket `From<String>`, D70; an opaque **`Cursor`** newtype for the keyset surface (single `#[serde(transparent)]` type, `from_raw` escape, D73); a structurally-sound **`ClientError`** — a `kind`(Transport/Decode/Api{status,code})-carrying `std::error::Error` with `Display`+`source()` (Arc-backed, stays `Clone`) + `code()`/`status()`/`message()` accessors, the embedded bridge preserving the wire status+code+message, D71) with an **opt-in in-process embedded bridge** (`ClientOptions::embedded` / `client_with` → emits `client::embedded(&engine)` over `based_runtime::Engine`, so an embedder writes zero `Transport` plumbing; referenced by path, no based-runtime dep; D62); `openapi` → OpenAPI 3.1 (D24); `migrate` → `schema.snap`/`up.mig` diff (D39) + `render_sql` per-dialect migration SQL (D41) + `sql_statements`/`content_hash` for apply (D42) + scope serialization (D50) + `@was` snapshot-authoritative renames (`Snapshot.renames` persisted → `rename table`/`rename column` steps → per-dialect `ALTER … RENAME`), the `raw(dialect)` escape step (`parse_raw_steps`), and the offline `drift` helper (D67). |
| based-facts | ✅ stable | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>` — the "show, don't write" facts (inferred inverses, join-key indexes, per-callable `$ctx` bags, resolved query shapes, scope contract), span-anchored, editor-string-scrubbed of internal refs (D50). |
| based-lsp | ✅ works (C4 complete) | tower-lsp server; recompiles on edit (unsaved buffers overlaid on disk), publishes diagnostics + inlay + hover + go-to-def (D43) + document symbols (D44) + completion (D45); per-file manifest resolution (D40); scope go-to-def/hover (D50); field-reference go-to-def + broad declaration hover + command-clickable inverse inlay (D51); find-references incl. filter calls + inverse back-edge, filter go-to-def (D52); rename + prepareRename reusing the reference index, back-edge excluded (D53); workspace symbols (⌘T) across every open project, fuzzy-filtered (D54); offline migration-drift diagnostic `W0108` + spent-`@was` `W0107` (diffs the latest `schema.snap` against the schema, no DB, D67); folding ranges (per multi-line decl body) + selection ranges (token→field→decl→file), both off the parsed decl spans (D68). |
| based-runtime | ✅ works (M6) | in-process engine (D18): `Compiled::load` reuses the front end + codegen lowering; `plan_query`/`plan_mutation` validate + bind (`?`/`$n` per dialect), `run_*` shapes rows / runs writes under one tx with declared-shape re-select on every surviving write (create-keyed D12 + update/soft-delete/restore where-keyed D58, read-your-writes); `nest_row` reassembles to-one sub-objects (dotted alias) + parses to-many JSON-array columns (`field[]`) into sub-object arrays (D55/D57); keyset pagination decodes the incoming `cursor` → `:keyset_` binds + mints the next opaque, checksum-validated cursor (`cursor`, D56). `serve::dispatch` is the wire core (maps `PlanError`/`DbError` to status; the machine `code` + message come from the errors' own `code()`/`Display`, one source of truth, D71); `http` the `based serve` listener (D21) with health/readiness/drain (D26); `embed` the socket-free door (D22); `idempotency` keyed write dedupe + fingerprint (D25/D31). Concrete drivers: `sqlite` (D27), `driver::MariaDb` + `ShardRouter` (D20/D35), `postgres` + `PgRouter` (D38; numeric binds are text-format so an i64 never mismatches an inferred `int4`, D59; result columns are read in binary format — uuid/timestamptz/date/jsonb decoded to their canonical strings, D61). Keyset/offset pagination + soft-delete/restore proven live on all three dialects (D59). Live-DB hardening (D65): per-dialect statement timeouts + bounded checkout wait on `PoolConfig`, drivers classify deadlock/serialization codes into `DbErrorKind::Deadlock` (mutation path retries the tx a bounded 5× with backoff) and pool saturation into `DbErrorKind::PoolExhausted` (fast 503), proven live on MariaDB/Postgres. `migrate` = live apply + ledger (D42). `based serve` is dialect-aware — the CLI branches on the manifest dialect to build the MariaDB/Postgres/SQLite backend (D66). Packaged as a container image (`docker/Dockerfile`, D66). *Open:* durable multi-instance idempotency store. |

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
- **Comments state what + why, briefly — never build-time narration.** Source is finished
  source, not a scratch pad: no "here's what I'm building" / WIP running commentary (it reads
  as unfinished and leads humans *and* agents off task). TODOs live in PLAN.md / roadmap `.md`,
  not inline, unless genuinely must-do/blocking. (The one-time cleanup of existing narration was Track F1, D69.)
- **Keep this file lean.** PLAN.md is the resume read; shipped-work narration goes to
  `PLAN-archive.md`, per-decision detail to `spec/decisions.md`. Add a one-line status + D# here,
  not a paragraph.
- `spec/principles.md` are the tiebreakers, in order. `spec/decisions.md` (with its topic index)
  resolves anything the prose left open.
