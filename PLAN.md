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

**Where things stand (as of D89):** the architecture milestones (M2–M6) are done, all three target
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
The 2026-07-10 strategic pivot — **Track N: async-native core → streaming → flagship axum example —
is now COMPLETE (N0–N3, D84–D89)** on the `async-native` branch, with the merge-to-main confidence
bar met (`make check` green end-to-end). **Track T (core DB parity) is now at T1–T5 done** —
**T3 atomic update expressions (D100)**, **T4 aggregations + group-by + having (D101)**, **T5 upsert
`create … on conflict update` + m2m-via-explicit-junction (D102)**. **The owner-flagged design
follow-ups are RESOLVED (D103–D107, owner-approved 2026-07-21); NF11 (D103, keystone — reworded
principle 8), NF9 opaque `raw(…)` column + exotic-index seam (D104), **NF13 named `tx` step
bindings `create … as name;` / `$name.field` replacing `^` (D107) — `^`/`E0170` retired**, NF7
`@was` self-consuming gen + teach-at-checkpoint (D105), and **NF8 honest snapshot-authoritative
`up.mig` contract + drift-refusal + editable surface (D106) are now shipped.** All D103–D107
follow-ups are done. **Track T is complete (T6 FK referential actions, D108)** — `@fk`/`@no_fk`
+ toml `foreign_keys` convention + the divergence-reason rule, DDL/snapshot/migration all three dialects,
SQLite cascade proven live. The **T5 m2m far-side flattening projection is now done (D109)** —
`courses = enrollments.course { … }` → a flat distinct `Vec<Course>`, junction hidden (two-level IN
subquery, runtime unchanged, proven live on SQLite); implicit-junction sugar stays rejected. Next is
Track T tier-2, with the SQLite incremental-FK rebuild left as the one explicit deferral. Batch-by-batch
history is in `PLAN-archive.md`.

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

## Track N — async-native pivot (owner decision 2026-07-10). ✅ COMPLETE (N0–N3, D84–D89)

**Strategic context.** The first real adoption target (the owner's workplace, the project's
proving ground) reviewed the pitch: the syntax landed well, **native async was repeatedly named
the core required feature**, streaming reads were wanted immediately, and the engine had to plug
into an app's *existing* async connection pool. The Rust web-backend market is effectively all
tokio, so the execution core was recolored to native async (no sync facade); the pure front end
(parse → sema → codegen → lowering) stays sync and runtime-free, CI-enforced. **All Track N work
is on the `async-native` branch; merge to `main` is the owner's call at demonstrated confidence**
(the bar — full gate + all three live suites + all examples green on the async core — is met:
`make check` green end-to-end). Delivery narration: `PLAN-archive.md`.

- **N0. ✅ (D84).** Async architecture design gate — every sync-era guarantee restated as an
  invariant with a named enforcement; sqlx as the driver layer; consuming-typestate `Tx`;
  stream-first `fetch`; CI-enforced coloring boundary.
- **N1. ✅ (D84 impl).** Native async execution core — the recolor (traits/dispatch/serve/client
  on sqlx 0.9), the cancel-safety acceptance gate (I2), and the BYO-pool seam
  (`from_pool` on all three backends), all proven live.
- **N2. ✅ (D85).** Streaming reads — `-> stream Shape` end to end: sema (E0200/E0201/E0202),
  streaming dispatch, NDJSON wire with mandatory terminal line, generated `RowStream` client +
  OpenAPI, live + cancel gates on all fronts.
- **N3. ✅ (D86–D89).** Flagship axum example + syntax appeal pass — the re-pitch artifact:
  design gate + coverage map (D86), ordered to-many nests (D87), guard seam + typed-client
  idempotency key (D88), the `examples/axum-helpdesk` schema/migrations/client/service (N3c/N3d,
  six engine bugs surfaced + fixed), and the N3e close-out — README (every command run-verified),
  final in-situ appeal re-audit (**zero grammar changes upheld**) + final coverage-map deltas
  (D89), CI wiring verified (`ci-example-helpdesk` runs inside `make ci-examples` / the `examples`
  CI job). Coverage policy stands: the quickstarts stay minimal; the helpdesk is the
  total-feature-coverage vehicle.

### Track N follow-ups (library gaps the flagship surfaced; owner picks order)

Each is one slice: symptom → seam → proposed fix. Detail/context in D89.

- **NF1. ✅ done (D93). `in` value-list form.** `status in (open, waiting, $extra)` parses
  (`Predicate::InList`; the single-bind `in $param` form unchanged), sema checks each element
  against the column (enum membership E0154, family E0151 — no new codes), and all three
  dialects lower to `IN (v, v, …)` with variants as wire values and `$param` elements bound.
  Variant nav/rename + fmt cover the new form. The helpdesk `open_states` filter now spells
  `not status in (resolved, closed)`. Proven unit + golden + live SQLite.
- **NF2. ✅ done (D94). Whole-query raw reads.** `{ raw`…`; }` as a query's whole body
  (`QueryBody::Raw`): the raw text IS the statement — `${param}` binds positionally,
  `{table}`/`{id}` interpolate the target, the declared (flat) shape types result columns
  by name, and the client/OpenAPI surface is identical to an engine-built query. W0102
  lints the soft-delete gap on the target *and* any soft-delete table the SQL mentions;
  un-composable combinations rejected loudly (E0210 untyped/bound params, E0211 `scoped`,
  E0212 `stream`, E0213 nested shape, E0214 `${ctx.…}`). fmt reprints the block
  byte-exactly; raw.md now spells the shipped contract. Proven unit + golden + live SQLite.
- **NF3. ✅ done (D95, D99). Guard re-entry no longer deadlocks — and is first-class.**
  `IdGen` mints by `&self` (`Send + Sync`; `SeqIdGen` on an atomic, `UuidGen` stateless), so
  the engine stores the generator bare — no mutex, nothing held across dispatch's awaits, and
  the deadlock shape is unrepresentable. Guards run before the mutation's connection checkout,
  so re-entry is pool-safe too. **D99 makes the handle first-class:** `GuardRequest::engine()`
  hands the dispatching engine to the guard, so a state-reading decision reads through the
  schema's own scoped/soft-deleted queries (`client::embedded(req.engine())`) instead of a
  captured pool + hand-written filters — no `OnceLock` back-reference. `Engine` is a `Clone`
  handle. The helpdesk `caller_can_close` drops sqlx and reads through `ticket`. Proven by the
  re-entry embed test (rewritten to `req.engine()`, timeout-bounded) + unchanged helpdesk smoke.
- **NF4. ✅ done (D98). `-> ok`: the shapeless ack return for destructive mutations** (owner
  pick 2026-07-20 over re-select-before-delete / both-forms / sema-reject-only). A real
  DELETE declares `-> ok` (contextual, return-position only): wire `{}`, unit-returning
  client method via a gated shared `Ack` type, OpenAPI `Ack` component. Shape + real DELETE
  is E0220; `-> ok` + a surviving write is E0221 (read-back stays mandatory where a row
  survives; raw rides along, first real DELETE is the primary model); `-> ok` on a query is
  E0222. A zero-row primary DELETE is D92's 404 `not_found` (rollback, claim released, no
  existence leak). Helpdesk `purge_comment` now `-> ok`, routed
  (`DELETE /admin/comments/{id}`) and smoke-proven (403/404-cross-tenant/200/404-repurge);
  typed live-SQLite embed proof included.
- **NF5. ✅ done (D97). `Page<T>` carries `with count`'s total.** `Page<T>` gains
  `total: Option<i64>` — `Some` exactly when the query declares `with count` (the wire
  has the field only then; `skip_serializing_if` keeps a re-served page wire-faithful).
  OpenAPI advertises `total` (int64) only on a counted query's page schema. Runtime was
  already correct. All four example clients + the embed-gate mirror regenerated; the
  helpdesk `/admin/tickets` now serves the total with no route change. Proven unit +
  typed embed round-trip + live SQLite + the extended helpdesk smoke.
- **NF6. ✅ done (D92). Zero-row surviving-write mutation → 404 `not_found`.** A re-select
  that reads back no row (wrong id, or a cross-tenant id under scope) now rolls the whole
  transaction back and surfaces `RunError::NotFound` → wire 404, stable code `not_found` —
  never a `200 null` the typed client can't decode. Same response for absent vs out-of-scope
  (no existence leak); a not-found releases the idempotency claim. Proven unit + live SQLite +
  the helpdesk smoke's new cross-tenant status-update assertion.
- **NF7. ✅ done (D105). `@was` lifecycle.** Shipped end to end (owner-approved 2026-07-21):
  **`gen` self-consumes** the spent `@was` — after `based migrate gen` writes a migration that emitted a
  `rename` step, it surgically strips that exact directive from the `.bsl` source and logs it
  (`removed spent @was("old") from Model.field …`); the rewrite is conservative (only a directive whose
  rename step was actually emitted, never a spent/still-live one) and idempotent. **Teach-at-checkpoint**
  — a single-table drop-one/add-one-same-family diff prints `if this renames X → Y, add @was("X") …` in
  `based migrate gen` stdout, the `W0108` drift note, and the `based migrate apply` destructive gate.
  `W0107` kept as the fallback lint for a hand-authored migration. Editor-rename `@was` insert already
  shipped (D80). New: `based-codegen::migrate::lifecycle` (`spent_was_edits`/`apply_spent_was`/
  `rename_hints`); wired through `based-cli` gen + apply and `based-lsp` drift; `PlannedMigration.rename_hints`.
  Proven: codegen unit (consume field/model, spent-not-consumed, hint fires/silent), CLI e2e
  (source rewritten, hint printed, re-gen no-op), runtime unit (hint on the destructive migration), LSP
  (W0108 note carries hint; W0107 still fires), `make check` green (live + examples + helpdesk smoke).
  Original writeup below (implementation context).
  Symptom: `@was` is a one-shot gen-time hint, so after `migrate gen` it is dead weight —
  either it lingers (W0107 cruft, a second commit to remove) or the author strips it
  pre-commit and no PR ever shows the gesture, so users never learn it and hand-write
  drop+add instead. Worse: renaming *without* `@was` yields a destructive drop+add with no
  "did you mean a rename?" anywhere (gen output, W0108 note, apply's destructive gate all
  silent). The durable record (`rename column` step in `up.mig` + snapshot) is fine; the
  authoring flow around it is not. Candidate direction (not yet decided): (a) gen
  self-consumes the spent `@was` from the `.bsl` after writing the migration — one command,
  no cruft; (b) teach at the checkpoint — when a diff drops column X and adds same-typed Y
  on one table, gen/W0108/destructive-gate output says `if this is a rename, declare
  @was("X") and re-gen` (this is the load-bearing piece: it gives `@was` the interactive
  prompt's self-revealing-at-the-moment-of-ambiguity property with zero prior knowledge,
  delivered over the run→read→edit→re-run loop agents actually have); (c) LSP quick-fix on
  W0107; (d) LSP `textDocument/rename` on a field inserts `@was("old")` as part of the
  rename edit — the LSP knows the old name, making the correct gesture the default for
  editor renames. Alternatives considered and rejected: interactive gen prompts (non-TTY
  harnesses hang; headless fallbacks default to drop+add; session answers make gen
  non-reproducible, breaking the drift re-check), keep-forever Terraform-`moved`-style
  hints (the `migrations/` ledger already holds transition history — pure changelog cost),
  rename only by hand-editing `up.mig` (stays legal as escape hatch). Spec seam:
  migrations.md E5 + decisions entry when resolved.

- **NF8. ✅ done (D106). Honest snapshot-authoritative `up.mig` contract + a real editable surface.**
  All six sub-items shipped: (a) the generated `up.mig` header now states the real contract (structural
  steps derive from `schema.snap`; editing a structural line has no effect; the editable surface is
  `raw(<dialect>)` lines + a hand-authored `down.mig`); (b) `apply`/`render` **refuse** (`MigrateError::
  UpMigDrift` / a CLI error) a migration whose structural `up.mig` residue diverges from the
  snapshot-derived SQL — the "verify-didn't-run" hole is closed (shared `migrate::up_mig_matches_snapshot`
  canonicalizes like `content_hash`, so cosmetic edits are tolerated and `raw` lines ride separately, still
  Tamper-guarded); (c) `.mig` + `.snap` tmLanguage grammars + VS Code language contributions (embedded SQL
  in raw blocks); (d) multi-line `raw(<dialect>)` backtick blocks (`parse_raw_steps`/`strip_raw_steps`/
  `has_raw_step` rewritten to a shared block-aware scanner — one-file artifact + hash contract kept, no
  sidecars); (e) `gen` prefills `down.mig` with real reverse SQL where mechanically reversible (add⇄drop,
  rename⇄rename, create⇄drop table) and a loud `-- … is irreversible …` comment otherwise (an all-comment
  placeholder counts as absent → roll-forward-only, not a silent no-op rollback); (f) migrations.md/raw.md
  document the raw/snapshot boundary and a `W0109` verify lint flags a raw step naming a modeled table.
  Tests: codegen units (header, multi-line raw round-trip, drift helper, down prefill, `raw_modeled_tables`
  word-boundary), runtime (UpMigDrift refusal at load, multi-line raw applies, Tamper reworked to a raw-line
  append), CLI (down.mig prefill, W0109 verify). Live via `make check`.

- **NF9. ✅ done (D104). Exotic column + index passthrough via `raw(…)`.**
  Shipped end to end: opaque column type `col: raw("geometry(Point,4326)")?` (per-dialect map
  `raw({ postgres: …, mariadb: … })` — canonical dialect-sorted in the snapshot so map order never
  churns a diff) with the literal type-string in DDL+snapshot (diff = string compare, so
  migrations/rebuilds/`@was` all work), an opaque client/OpenAPI value (`String`), excluded from
  create/update unless nullable/defaulted, filter/sort/group/aggregate rejected except via the raw
  value/predicate leaf. Two exotic-index tiers: `@index(col) using <method>` (btree/hash/gist/spgist/
  gin/brin; MariaDB fulltext/spatial — Postgres leading `USING`, MariaDB inline kind/trailing `USING`)
  and opaque `@index raw("…")` (content-hashed name, always a standalone `CREATE INDEX`, string-compare
  diff). Two-phase check: dialect-free `check` (E0271/E0273/E0274 + unknown-method/dialect) plus a new
  `check_target(&schema, dialect)` for target-decided errors (E0270 missing map dialect, E0272 method
  unavailable on the target — e.g. every method on sqlite); CLI runs it with the manifest dialect, LSP
  with the resolved project dialect. Exotic/opaque indexes never satisfy E0260 nor trip W0104. Proven
  live on SQLite (opaque column + opaque index in real DDL, create-omits + read-back bare + via a raw
  leaf) plus unit (+/− sema, DDL all three dialects, snapshot round-trip + diff, client/openapi),
  conformance golden `raw_opaque`, fmt round-trip. **Standing convention upheld: `sql` is never a
  keyword/marker; `raw` is the one spelling (D96).** Original writeup below.
  Problem: the primitive set (`text int bool timestamp date json uuid float decimal`) is
  closed — a column whose DB type we don't model (PostGIS `geometry`, `tsvector`, `inet`,
  vendor JSON variants) **cannot be declared at all**. First-class geo support is not
  warranted (niche, per-dialect), but the current cliff is total: the user's only move is a
  raw migration adding the column behind the schema's back, which (1) makes the snapshot
  blind on a modeled table, (2) gets the column silently DROPPED by any future sqlite
  table-rebuild (rebuild recreates from snapshot), (3) excludes it from every generated
  surface. That is the "throw the whole system away" failure mode for one field. Candidate
  direction (not yet decided): a Prisma-`Unsupported("…")`-style opaque column type — e.g.
  `location: sqltype("geometry(Point,4326)")?` (spelling TBD; likely per-dialect map since
  type names differ) where the engine stores the literal type string in DDL + snapshot
  (diff = string compare, migrations/rebuilds/`@was` all work), the typed client treats the
  value as opaque (bytes/string, or excluded from create/update unless nullable/defaulted —
  Prisma's rule), filters/sorts on it are rejected except via the existing raw predicate
  hatch, and functions over it (`ST_Area(...)`) go through raw-value-in-shape / raw query
  (raw.md — already sufficient for the *read* side today). Result: one opaque field
  degrades gracefully — CRUD on the rest of the model, migrations, and the drift check all
  stay in-system; raw stays at the leaves (principle: raw at the leaves, never the
  structure). **Indexes are in scope (owner, 2026-07-16): an opaque column you can't index
  is dead weight** — a geometry column without GIST is unusable, same for `tsvector`+GIN.
  Today `@index` is columns+`unique` only and `IndexSnap` records `{name, columns, unique,
  inferred}` — no access method, expressions, opclasses, or predicates — so an exotic index
  hits the identical cliff, and a raw-migration index is the same trap as a raw column
  (orphaned on model drop, lost on sqlite table-rebuild, invisible to the index lints).
  Same seam, two tiers: (i) `@index(location) using gist` — a `using <method>` token
  (gist/gin/brin/hash…; MariaDB `FULLTEXT`/`SPATIAL`), snapshot-recorded, per-dialect
  validity checked loudly (sqlite lacks most methods → error at gen, not silent skip);
  (ii) an opaque index form for the long tail (expression indexes, opclasses, partial
  `WHERE`) recorded in the snapshot as a literal string, diffed by string compare — exactly
  the opaque-column treatment, so create/drop/rebuild lifecycle stays in-system. Spec seam:
  models.md Types + indexing.md + raw.md + migrations.md; decisions entry when resolved.

- **NF10. ✅ done (D91). Derived facts anchor narrowly, never at a whole decl.** The
  inferred-index fact anchors at the forward member whose FK it covers; ctx-requirement /
  resolved-query facts at the callable's name ident — so hover facts no longer bleed into
  every token inside the decl. NF11, if adopted, retires the inferred-index fact; the
  narrow ctx/resolved-query anchoring stands regardless.

- **NF11. ✅ done (D103, keystone). Inferred indexes + implicit `id` → explicit-in-source.**
  Shipped: a traversed-but-unindexed join key (and a scanning root filter) → error **E0260**
  + one-key LSP autofix (insert `@index <field>`); a model with no `id` → error **E0261** +
  autofix (insert `id: Id`); both fire in `based check`, not editor-only. The inferred-index
  machinery is retired end to end (`RModel.inferred_indexes`, the `indexes.rs` baseline, the
  `inf_` DDL naming, `IndexSnap.inferred`, the `InferredIndex` fact + inlay) — the DDL's indexes
  are exactly the written `@index` set, still rendered soft-delete-leading on a `@soft_delete`
  model. Conformance goldens + `spec/examples/commerce` + the four `examples/*` schemas gained
  explicit `@index`/`id` lines (example migrations regenerated). Follow-up landed: **`@no_id("reason")`**
  — the E0261 opt-out for a genuinely keyless legacy table (mandatory reason `E0262`); it forfeits the
  id-keyed ops, each a loud error — keyset-without-unique-sort `E0263`, create-read-back-without-unique
  `E0264`, relation-to-keyless `E0265`; codegen drops the PK + id tiebreaker, and the create reads back by
  a unique column (proven live on SQLite). Original resolution + writeup below. Resolution (owner 2026-07-21): a traversed-but-unindexed join key → error
  **E0260** (was `W0103`) + one-key LSP autofix (`unindexed(…)` stays the visible opt-out); a model
  with no `id` → error **E0261** + autofix; both fire in `based check`, not editor-only. **Principle 8
  reworded** (its inferred-index example inverts; principle 2 governs). `IndexSnap.inferred` / `inf_`
  naming / `RModel.inferred_indexes` / the `InferredIndex` fact+inlay all retire; the written `@index`
  is still rendered soft-delete-leading (a rendering, not a second index); inverse pairing stays a
  shown fact. **Resolves D102's m2m fork: no implicit-junction sugar** (silent DDL), junction FK
  indexes are explicit; the far-side flattening projection is now done (D109). Fallout on landing: goldens +
  `spec/examples/commerce` + the four `examples/*` schemas gain explicit `@index`/`id` lines. Original
  writeup below. Owner position: silent engine-created DDL disobeys principles — an
  index has real write/disk cost and is invisible in a PR (hard priority 3: reviewer
  confirms design by reading; editor-only facts never reach review), and principle 2
  ("nothing consequential is true by omission") outranks principle 8's "show, don't write"
  carve-out, which today *names inferred indexes as its example*. Same verdict on the
  implicit `id` field (models.md "id implicit", D1-era): same silence, same fix. Direction:
  stop inferring — a traversed relation join-key with no covering `@index`, and a model with
  no declared `id`, become **compiler errors** (not IDE-only, so the CLI is equally honest)
  with an **LSP code-action autofix** that inserts the exact line (`@index <field>` / `id`
  line), so the add is zero-thought; the index error gets an explicit opt-out token for the
  deliberate no-index case (unindexed-flavored, visible-dangerous per principle 1).
  Consequences: principle 8 needs rewording (its inferred-index example inverts); the
  `IndexSnap.inferred` tier + `inf_` naming and the InferredIndex fact/inlay retire;
  models.md Defaults + the implicit-fields decision revisit; conformance goldens + examples
  gain the explicit lines. Boundary: inferred *inverse pairing* stays as-is — the inverse
  field is written in source and only the unambiguous pairing is derived (passes principle
  2's elision test), so it remains a shown fact. Spec seam: principles.md, models.md,
  indexing.md; decisions entry when resolved.

- **NF12. Inline raw SQL has no SQL highlighting in `.bsl` (owner-observed 2026-07-16).**
  The tmLanguage `#raw` rule scopes backtick bodies as one string
  (`string.quoted.other.raw.bsl`, editors/vscode/syntaxes/bsl.tmLanguage.json) — no embedded
  grammar, so ``raw`concat(first, ' ', last)` `` renders as a flat string. **Requirement
  (owner, 2026-07-16): the SQL highlighting dialect must match what the user defines as the
  dialect in `based.toml`** — not a generic-SQL guess, no fallback layering. Mechanism
  consequence: a tmLanguage grammar is static and cannot read project config, so the
  grammar alone cannot satisfy this; the component that already knows the manifest dialect
  is the LSP (it walks up to `based.toml` and compiles per-project — compile.rs
  `find_manifest_root`/`compile_manifest`). Direction: **LSP semantic tokens** over the raw
  backtick interiors — based-lsp tokenizes the embedded SQL with a per-dialect table
  (keywords, string/identifier quoting — where dialects genuinely differ, e.g. MariaDB
  backtick identifiers vs postgres double-quotes and `$$` strings — comments, numbers,
  `${param}` interp as its own token) and the editor renders those over the TextMate
  baseline; works in any LSP editor, keeps the extension a thin client. `.mig` raw lines:
  same semantic-token treatment once NF8(c) gives `.mig` a language contribution — there
  each line's `raw(dialect)` token names its dialect explicitly (agrees with the manifest
  in a single-dialect project; the line token is the more specific signal if they ever
  differ). The marker is `raw` (NF14, D96).

- **NF14. ✅ done (D96). Raw marker renamed `sql` → `raw`.** The backtick escape hatch is
  spelled ``raw`…` `` everywhere — grammar.ebnf, parser keyword, fmt printer, LSP keyword
  completion, tmLanguage keyword list, spec examples (raw.md/queries.md/soft-delete.md),
  conformance goldens, helpdesk schema. No back-compat alias: `sql` as a marker is now an
  ordinary parse error. Greppability carries over unchanged (``raw` ``).

- **NF15. ✅ done (D90). Param bindings are first-class editor references.** Binding idents
  (`-> edge` / `op col`) folded into the LSP reference walk rooted at the query's target
  model — rename (the bug: a field rename silently skipped `has tags`), find-references,
  go-to-def, and field hover all see them; hovering a binding (ident or operator token)
  states the predicate it generates, op-glossed; an unbound bare/inline param hovers its
  derived same-name equality; tmLanguage keyword/type lists audited against grammar.ebnf.
  Out of scope on purpose (detail: D90): rename does not rewrite an unbound param's name
  (wire contract; the miss is a loud E0111, not silent). The irrelevant resolved-query/ctx
  hover sections remain NF10's whole-decl-span bleed, filed there.

- **NF13. ✅ done (D107). Named `tx` step bindings replace `^`.** Bind a step with
  `create … as name;` and reference a column of it from **any** later step as `$name.field`; `$`
  unifies to "a value bound in this callable" (params + `$ctx` + step bindings), single-assignment
  and field-access only (principle 5 intact). **`^` removed entirely — no back-compat shim**:
  `Tok::Caret` / `Value::Back` / `BackRef` / `BackCtx` and `E0170` are all **retired**; a `^` in
  source is now an `E0001` parse error whose message points at `create … as <name>;` + `$name.field`.
  A binding shadowing a param or duplicating another is **E0280**; an unbound or forward-referenced
  `$name` is **E0281** (an unknown field on the bound step reuses `E0111`). Shipped end to end —
  grammar.ebnf + parser (`as name`, no `Caret`), AST (`Create.binding`, `Value::Back` gone), sema
  (Bindings env reaching any prior step; E0280/E0281), codegen (name-addressed `BackCtx` map; same
  `:id_<step>` lowering, any prior step), based-fmt (`as name` round-trip), and the editor surface
  (a binding is a go-to-def / find-refs / rename site; `$name.field` resolves to it; hover names its
  model). Helpdesk `open_ticket` + tests migrated off `^`. Verified: parser +/- (as-binds; bare `^`
  errors → `as`), sema +/- (E0280 shadow/dup, E0281 unbound/forward, happy 3-step tx where step 3
  reaches step 1), codegen SQL (3-step reach-any-prior-step), conformance golden `tx_bindings`, fmt
  round-trip, LSP nav/rename/hover, and **live** via `make check` (the helpdesk `open_ticket` tx ran
  green against live Postgres/MariaDB). Original writeup below.
  Today a `tx` step back-references the prior step only via `^`
  (`create Comment { ticket = ^.id, … }`, mutations.md). Owner: the tx form is neat and
  stays, but `^.id` oversteps the keystrokes-vs-intuition line — unintuitive, ungreppable,
  and it only reaches the *immediately preceding* step, so a 3-step tx referencing step 1
  is unwritable. Proposal: named bindings — bind a step's produced row and reference it as
  `$name`, unifying `$` as "a value bound in this callable" (params + step bindings; a
  binding shadowing a param is an error). Spelling note: the bare trailing form
  (`create Ticket { … } ticket;`) is two adjacent bare tokens where the gap is the syntax —
  banned by principle 3 — so it wants a keyword, e.g. `create Ticket { … } as ticket;`.
  No Turing-creep: single assignment, no rebinding, reference is field-access only
  (`$ticket.id`) — principle 5 intact. Decide `^`'s fate: leaning **remove** (one way to
  say it; bindings strictly subsume it) with a parse error suggesting `as`. Spec seam:
  mutations.md Atomic groups + grammar.ebnf; parser/sema/codegen + helpdesk example
  (`open_ticket`) + conformance goldens follow.

## Track T — core DB feature parity (owner-approved 2026-07-09; T1–T6 done, tier-2 next)

A confirmed 6-item queue closing the gap to a general DB-first DSL (commerce is only a *named*
example, not the domain). **T1–T6 all done (enum D82, decimal/float D83, atomic update exprs D100,
aggregations+group-by+having D101, upsert + m2m-via-explicit-junction D102 + **far-side flattening
projection D109**, FK referential actions D108).** One slice remains deferred: the T6 SQLite
incremental-FK table rebuild (from-scratch FKs already work on SQLite). Next is **tier-2**
(for-update locking, computed shape fields, `distinct`, time/bytes types). Worked in order; each
iteration marks its item + D#.

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
- **T3. ✅ done (D100). atomic update expressions** — `update … { qty = qty + $delta }`, a self-referential
  arithmetic SET over the model's numeric columns/params, lowered to a real SQL `SET col = (…)` computed
  server-side (closes the lost-update gap), never a re-read-then-write. `+ - * /` (precedence + parens),
  numeric family only (int/float/decimal, D83), **update-only** (a `create` has no row to reference).
  AST `Assign.value: AssignRhs` (`Value` | `Arith`) + new lexer tokens `+ - * /`; column operands read
  through the ordinary qualified-value path (all three dialects accept it on a SET RHS). New codes E0230
  (arith in `create`) / E0231 (non-numeric operand); a numeric expr → text column is the ordinary E0153.
  fmt reprints with minimal parens; RHS column/param refs are go-to-def/find-refs/rename sites. Proven
  unit (sema +/− , codegen SQL all 3 dialects) + conformance golden + fmt round-trip + **live SQLite**
  read-your-writes (100+25→125, 125−5→120, server-side).
- **T4. ✅ done (D101). aggregations + group-by + having** — an *aggregate shape* projects
  `count()`/`sum`/`avg`/`min`/`max` (`out = count()` / `= sum(total)`, `ShapeValue::Agg`) over groups;
  a query pairs it with `group by (cols)` / `having (pred)` / `order` (`Clause::GroupBy`/`Having`). Every
  non-aggregate projected column must be grouped (E0242); no `group by`/`having` = one whole-table row.
  Codegen lowers to a real `GROUP BY`/`HAVING` SELECT, row filter (`where` + soft-delete + `@scope`)
  narrowing *before* grouping; aggregates dialect-**cast** so decode is deterministic (count→int; sum
  keeps the family, cast back where Postgres/MariaDB widen `SUM(int)`, SQLite renders decimal-sum as text;
  avg→double; min/max native) — runtime unchanged. `having`/`order` inline the numeric aggregate (not the
  SELECT alias/decimal-text-cast). Result typing: count→`i64`, sum/min/max→`Option<col-type>`, avg→
  `Option<f64>` (client + OpenAPI). Boundaries (all loud): aggregate shape is flat + never nested/a
  mutation return (E0245); aggregate query isn't paginated (E0244); `group by`/`having` need an aggregate
  shape (E0243); bad agg call E0240, ineligible column E0241. Editor: `group`/`by`/`having` keywords +
  agg-func completions; group-by/agg-arg columns are nav/rename sites. Codes E0240–E0245. Proven unit
  (sema +/−, codegen SQL all 3 dialects, client typing) + sema conformance golden + fmt round-trip +
  **live SQLite** GROUP BY/HAVING (soft-delete-before-group, count/sum/avg/max, ordered groups).
- **T5. upsert ✅ done (D102); m2m specced (explicit-junction pattern works) — one slice open.**
  **Upsert:** `create <M> { … } on conflict (target) update { … }` — per-dialect `ON CONFLICT
  (cols) DO UPDATE` (Postgres/SQLite) / `ON DUPLICATE KEY UPDATE` (MariaDB), the `update` branch
  reusing D100 atomic arithmetic (`hits = hits + 1` composes on the stored value); read-back keyed
  on the conflict target (a conflict path keeps the existing row's id, so keying on it would
  miss). Five codes: E0250 (target not a unique key), E0251 (branch moves the key), E0252 (target
  unset by the create), E0253 (soft-delete model), E0254 (scoped target omits the scope column —
  cross-scope-write guard). Runtime unchanged (conflict-key placeholders are params/`$ctx` already
  bound). Proven unit (parser/sema +/−/codegen all 3 dialects/fmt) + sema conformance golden +
  **live SQLite** (insert→conflict→conflict composing 1→2→3→4). **m2m:** modeled by an explicit
  junction model (two forward edges + two to-many inverses — no new syntax; already works via
  L1/D57). **Far-side flattening projection ✅ done (D109):** `courses = enrollments.course { … }`
  → a flat *distinct* `Vec<Course>`, the junction hidden (a two-level correlated `IN` subquery —
  `FROM far WHERE far.id IN (SELECT junction.far_fk …)` — distinct-on-PK for free on all three
  dialects; reuses D57's json-agg + `field[]` marker, so the runtime is unchanged; junction *and*
  far `@scope`/`@soft_delete` ride the right level; E0300–E0302; proven live on SQLite). This
  closes the last open T5 slice. **Implicit-junction sugar (`Course[] <-> students`) stays
  rejected** (D103 — an engine-generated join table is PR-invisible DDL). Detail: D109, relations.md.
- **T6. ✅ done (D108). FK referential actions** — opt-in DB `FOREIGN KEY` constraints with
  `on_delete`/`on_update` cascade/restrict/set_null/no_action, all visible in source. `@fk(…)` opts a
  forward relation in (+ actions); `@no_fk` opts out (one edge or a whole model); toml
  `[schema] foreign_keys = "all" | "none"` (default `none`) sets the project convention. The load-bearing
  **divergence-reason rule** (spelled like `@no_id("reason")`): a reason is required exactly when a
  decorator flips FK presence *against* the convention (`E0295`), and a decorator that restates it is a
  `W0110` redundancy lint — checked in a manifest-dependent pass (`check_foreign_keys`) mirroring D104's
  `check_target` (CLI = manifest value, LSP = resolved project value). Structural checks (convention-free):
  E0290 (bad target), E0291 (custom-join), E0292 (fk+no_fk), E0293 (set_null on required), E0294 (unknown
  action). DDL emits the FK inline on all three dialects (SQLite too); the `foreign_keys` pragma is on so
  cascade enforces — proven **live on SQLite** (bad-parent insert rejected, parent delete cascades the
  child). Resolved FKs ride `schema.snap` (`ForeignKeySnap`), so an FK add/remove/change diffs into an
  `add/drop foreign_key` step (PG/MariaDB `ALTER … ADD/DROP CONSTRAINT`; **SQLite alter-FK is an honest
  raw(sqlite)-rebuild marker, not a silent skip** — full rebuild engine deferred, see follow-up). Tests:
  parser +/−, sema +/− both toml directions incl. both redundancy lints, DDL golden ×3, snapshot
  round-trip + diff, fmt round-trip, conformance golden `fk_referential`, live cascade. `make check` green.
  - **T6 follow-up (deferred):** SQLite in-place FK add/drop needs the 12-step table-rebuild engine
    (same gap as SQLite `alter column`); today it emits a loud `raw(sqlite)` rebuild marker. From-scratch
    `create table` already carries FKs inline on SQLite, so init + `based gen sql` work — only an
    *incremental* FK change on an existing SQLite table needs the rebuild.

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
- **H10. CI-infra hardening (surfaced 2026-07-16: the Docker VM disk filled — 5,943 anonymous
  volumes / 658GB leaked across historical CI runs — and the mariadb CI container died instantly
  on start while `make check` hung or failed opaquely).** Three fixes:
  (a) Makefile teardown must use `docker rm -fv` (not `-f`) so each run's anonymous DB volumes
  are removed with the container instead of leaking one per run;
  (b) the runtime test suites' in-process `wait_ready` poll retries forever — give it a deadline
  so a dead DB fails the suite fast with a clear message instead of hanging it;
  (c) `ci/wait-for-db.sh`'s TCP-accept readiness check can pass spuriously right after a container
  dies (observed with OrbStack port forwarding) — verify the container is still running (or use a
  protocol-level ping) before declaring ready.

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
| based-ast | ✅ stable | AST mirrors grammar.ebnf node-for-node. No logic. `Decl::Enum` (variants carry an optional `= STRING\|INT` value) + `DefaultVal::Variant` (D82); `Primitive::Float` + `Primitive::Decimal { precision, scale }`, and `Literal::Decimal(String)` (a fractional literal's exact source text; replaced `Literal::Float`) so a decimal default/value is byte-exact (D83). `Assign.value` is now `AssignRhs` (`Value` \| `Arith { lhs, op, rhs }`) + `ArithOp` for atomic update expressions, with an `as_value()` accessor for the single-value sites (D100). `ShapeValue::Agg(AggCall)` (aggregate shape field) + `Clause::GroupBy`/`Clause::Having` for aggregations (D101). `WriteStmt::Create` gains `conflict: Option<OnConflict>` (upsert target + update branch, D102). |
| based-diagnostics | ✅ stable | `Diagnostic` + `Severity`; stable codes; builder API. |
| based-manifest | ✅ works | `based.toml` + `**/*.bsl` glob (D5). `$ctx` is inferred in sema, not declared here (D4). `[schema] foreign_keys = "all"\|"none"` (default `none`) — the FK-constraint convention, threaded into sema/codegen (D108). |
| based-parser | ✅ works | hand-written RD parser + lexer; golden + unit tests. Parses `decimal(p, s)` (optional args → `decimal_args`, bare = `(38, 9)`) + `float`; a `FLOAT` token → `Literal::Decimal` exact text (D83); an update assign RHS parses as a precedence-climbing arithmetic expression (`+ - * /`) over `value` leaves, new lexer tokens `+ - * /` (D100); a shape value `= count()`/`= sum(col)` parses to `ShapeValue::Agg`, and `group by (cols)`/`having (pred)` as query clauses (D101); a create's `on conflict (cols) update { … }` upsert tail (D102); field-level `@fk[("reason", on_delete: …, on_update: …)]` / `@no_fk[("reason")]` on a forward relation + model-level `@no_fk` (D108). |
| based-fmt | ✅ works | canonical `.bsl` formatter (D78): `format_source` = parse → AST pretty-print, a pure function of the AST except lexer-discarded comments (recovered from source, re-emitted in slot). Deterministic + idempotent + reparse-stable; byte-exact no-op over every committed schema (type-column/inverse-ref alignment, width-gated inline shapes, clause-count query blocks, minimal-paren predicates, minimal-paren atomic update expressions D100, aggregate shape values + `group by`/`having` clauses D101, upsert `on conflict (…) update { … }` tail D102). Shared by `based fmt` + the LSP `formatting` handler. |
| based-sema | ✅ stable | resolution + checks + lints + `CheckedSchema` IR (incl. `@was` rename directive on `RModel`/`RMember` + `E0190`/`E0191`, D67; enum decls → `REnum` with inferred `kind` (string/int) + valued variants, kind/dup/ordered-op/membership checks E0104/E0106/E0154/E0155/E0156/E0157/E0158, D82); decimal/float numeric family + `decimal(p,s)` range / decimal-default validity (E0159, D83); atomic update expressions — numeric-only arithmetic RHS, update-only (E0230), non-numeric operand E0231, numeric-expr→text column the ordinary E0153 (D100); aggregate shapes + `group by`/`having` — agg-call/arg validity (E0240/E0241), group-by consistency + order/having references (E0242), `group by`/`having` context (E0243), no-page (E0244), flat/non-nested/non-mutation-return composition (E0245) (D101); upsert `on conflict` validation — target-is-unique-key (E0250), branch-moves-key (E0251), target-unset-by-create (E0252), soft-delete-model (E0253), scoped-target-omits-scope-col (E0254) (D102); explicit-in-source index/PK — a traversed-or-scanned join/filter with no covering `@index` is `E0260` (was the `W0103` warn + the retired `inferred_indexes` baseline), a model with no `id` is `E0261`, both carrying a one-key autofix `Fix`; the `@no_id("reason")` keyless opt-out (mandatory reason `E0262`) forfeits the id-keyed ops — keyset-without-unique-sort `E0263`, create-read-back-without-unique `E0264`, forward-relation-to-keyless `E0265` (D103); opt-in FK constraints — the convention-free structural checks in `check` (`validate_fk`: bad target E0290, custom-join E0291, fk+no_fk E0292, set_null-on-required E0293, unknown action E0294) plus the manifest-dependent `check_foreign_keys(&schema, foreign_keys)` divergence-reason pass (mirrors D104's `check_target`) — a decorator flipping FK presence against the toml convention needs a reason (E0295), one that restates it is a `W0110` redundancy lint; `MemberKind::Forward` carries an `FkDecl`, `RModel::resolved_fk` resolves presence+actions, `ForeignKeys` enum (D108). Detailed behaviour in the next section. |
| based-cli | ✅ works | `based check`; `based fmt [--check]` (canonical formatter over the manifest glob, in-place or check-only, D78); `based gen sql\|client\|openapi`; `based facts [--json]`; `based migrate gen\|render\|apply\|status\|verify`; `based serve`. Structured top-level `CliError` (`Display` + `source()` chaining; exit 2 usage/config, exit 1 operational failure) reusing the runtime's typed `MigrateError`/`DbError` as the cause; rustc-style parse/sema diagnostics via `render.rs` (D72). |
| based-codegen | ✅ stable | `sql::ddl\|dml\|mutations` → dialect-aware DDL/SELECT/INSERT-UPDATE-DELETE (MariaDB/SQLite/Postgres, D28/D29) through one `Dialect` quoting/type seam; declared-shape re-select on every surviving write (create-keyed D12 + update/delete/restore where-keyed D58); nested to-one shape sub-objects (D55) + to-many nested arrays via correlated-subquery JSON aggregation incl. self-ref aliasing (D57), each with a nest-reached scoped child's `@scope` injected into the join `ON` / subquery `WHERE` (D81) + keyset-cursor pagination (lexicographic `WHERE` + hidden `__keyset_` columns, D56); `client` → typed Rust client (nested `Vec<…>` for to-many, paginated inputs carry a typed `cursor`/`offset`, D56/D57; **per-entity phantom-typed ids** `Id<entity::M>` — transparent wire, `from_raw` escape, no blanket `From<String>`, D70; an opaque **`Cursor`** newtype for the keyset surface (single `#[serde(transparent)]` type, `from_raw` escape, D73); a structurally-sound **`ClientError`** — a `kind`(Transport/Decode/Api{status,code})-carrying `std::error::Error` with `Display`+`source()` (Arc-backed, stays `Clone`) + `code()`/`status()`/`message()` accessors, the embedded bridge preserving the wire status+code+message, D71) with an **opt-in in-process embedded bridge** (`ClientOptions::embedded` / `client_with` → emits `client::embedded(&engine)` over `based_runtime::Engine`, so an embedder writes zero `Transport` plumbing; referenced by path, no based-runtime dep; D62); `openapi` → OpenAPI 3.1 (D24; an enum field → a string- or integer-schema with `enum: [...]`, D82); enum columns →
a text-or-integer column + named `CHECK (col IN …)` in DDL (string or numeric kind, D82), a real Rust enum in the
client (string → serde-rename incl. name≠value; int → discriminants + hand-rolled i64 serde, no new dep, D82),
and `enum(…)`/`enum:int(…)` in the neutral snapshot so a variant or kind change diffs (D82); decimal/float scalars → per-dialect `DECIMAL(p,s)`/`NUMERIC(p,s)`/`TEXT`(SQLite) + `DOUBLE`/`REAL` (`sql_type` now returns `String`), the client emits `rust_decimal::Decimal` (JSON-string wire via `serde-str`) + `f64`, openapi string-decimal/number-double, and `decimal(p,s)`/`float` in the neutral snapshot (D83); `migrate` → `schema.snap`/`up.mig` diff (D39) + `render_sql` per-dialect migration SQL (D41) + `sql_statements`/`content_hash` for apply (D42) + scope serialization (D50) + `@was` snapshot-authoritative renames (`Snapshot.renames` persisted → `rename table`/`rename column` steps → per-dialect `ALTER … RENAME`), the `raw(dialect)` escape step (`parse_raw_steps`), and the offline `drift` helper (D67); an atomic update expression (`qty = qty + $n`) lowers to a real SQL `SET col = (…)` — each binary node parenthesized, column operands read through the qualified-value path, all three dialects (D100); an aggregate query lowers to a `GROUP BY`/`HAVING` SELECT (row filter before grouping; aggregates dialect-cast for deterministic decode; `having`/`order` inline the numeric aggregate), the client types agg fields (count→`i64`, sum/min/max→`Option<col-type>`, avg→`Option<f64>`) + OpenAPI schemas (D101); an upsert `create … on conflict` lowers to per-dialect `ON CONFLICT (cols) DO UPDATE` / `ON DUPLICATE KEY UPDATE` (bare conflict-SET columns via `Select::with_bare_cols`) with a conflict-target-keyed declared-shape re-select (`RetKey::Conflict`, not the discarded generated id) (D102); the inferred-index baseline retired (D103) — DDL + snapshot emit exactly the written `@index` set (`IndexSnap.inferred` + the `inf_` naming gone), a non-unique declared index on a soft-delete model rendered predicate-leading. Opt-in FK constraints (D108): `sql::ddl_with`/`Snapshot::from_schema_with` thread the `foreign_keys` convention (old `ddl`/`from_schema` keep the safe `none` default), FKs emit inline `CONSTRAINT fk_… FOREIGN KEY … REFERENCES …[ON DELETE/UPDATE …]` on all three dialects (SQLite too), a resolved FK is a `ForeignKeySnap`/`fk` line in `schema.snap` so a change diffs into `add`/`drop foreign_key` steps (PG/MariaDB `ALTER … ADD/DROP CONSTRAINT`/`DROP FOREIGN KEY`; SQLite alter-FK = honest raw(sqlite)-rebuild marker, not a silent skip). |
| based-facts | ✅ stable | pure `facts(&CheckedSchema, &[Decl]) -> Vec<Fact>` — the "show, don't write" facts (inferred inverses, per-callable `$ctx` bags, resolved query shapes, scope contract), span-anchored, editor-string-scrubbed of internal refs (D50). The `InferredIndex` fact retired (D103): an index is written in source, not shown. |
| based-lsp | ✅ works (C4 complete) | tower-lsp server; recompiles on edit (unsaved buffers overlaid on disk), publishes diagnostics + inlay + hover + go-to-def (D43) + document symbols (D44) + completion (D45); per-file manifest resolution (D40); scope go-to-def/hover (D50); field-reference go-to-def + broad declaration hover + command-clickable inverse inlay (D51); find-references incl. filter calls + inverse back-edge, filter go-to-def (D52); rename + prepareRename reusing the reference index, back-edge excluded (D53), extended to params, `$ctx` bag fields, callable names, and `@was`-aware physical rename of a live column/table (D80); workspace symbols (⌘T) across every open project, fuzzy-filtered (D54); offline migration-drift diagnostic `W0108` + spent-`@was` `W0107` (diffs the latest `schema.snap` against the schema, no DB, D67); folding ranges (per multi-line decl body) + selection ranges (token→field→decl→file), both off the parsed decl spans (D68); whole-document `formatting` returning one full-document `TextEdit`, delegated to a thin `Snapshot::format_document` over `based_fmt` (D78); enum-variant navigation — go-to-def/find-refs/rename (enum-local) + hover in value/default position, plus enum type-ref go-to-def/hover + document/workspace symbols for the enum + its variants (D82); `group`/`by`/`having` keywords + aggregate-func completions, and group-by/aggregate-arg columns as go-to-def/find-refs/rename sites (D101); **code-action quick-fixes** for `E0260`/`E0261` — a diagnostic's `Fix{model,line}` becomes a one-key `@index <field>` / `id: Id` insertion at the model body's top (`code_action` handler + `Snapshot::member_insert_edit`; capability advertised) (D103). |
| based-runtime | ✅ works (M6) | in-process engine (D18): `Compiled::load` reuses the front end + codegen lowering; `plan_query`/`plan_mutation` validate + bind (`?`/`$n` per dialect), `run_*` shapes rows / runs writes under one tx with declared-shape re-select on every surviving write (create-keyed D12 + update/soft-delete/restore where-keyed D58, read-your-writes; an atomic update expression's `$param` operand binds at the target column's numeric family, D100); `nest_row` reassembles to-one sub-objects (dotted alias) + parses to-many JSON-array columns (`field[]`) into sub-object arrays (D55/D57); keyset pagination decodes the incoming `cursor` → `:keyset_` binds + mints the next opaque, checksum-validated cursor (`cursor`, D56). `serve::dispatch` is the wire core (maps `PlanError`/`DbError` to status; the machine `code` + message come from the errors' own `code()`/`Display`, one source of truth, D71); `http` the `based serve` listener (D21) with health/readiness/drain (D26); `embed` the socket-free door (D22); `idempotency` keyed write dedupe + fingerprint (D25/D31). Concrete drivers: `sqlite` (D27), `driver::MariaDb` + `ShardRouter` (D20/D35), `postgres` + `PgRouter` (D38; numeric binds are text-format so an i64 never mismatches an inferred `int4`, D59; result columns are read in binary format — uuid/timestamptz/date/jsonb decoded to their canonical strings, D61; a `numeric`/`decimal` column decoded from its binary base-10000 form to an exact string via `pg_numeric`, D83). A decimal rides the runtime as its exact wire string end to end (`Family::Text`; `rust_decimal` stays out of based-runtime) — SQLite stores it as `TEXT`, MariaDB returns it as text, Postgres via `pg_numeric` (D83). Keyset/offset pagination + soft-delete/restore proven live on all three dialects (D59). Live-DB hardening (D65): per-dialect statement timeouts + bounded checkout wait on `PoolConfig`, drivers classify deadlock/serialization codes into `DbErrorKind::Deadlock` (mutation path retries the tx a bounded 5× with backoff) and pool saturation into `DbErrorKind::PoolExhausted` (fast 503), proven live on MariaDB/Postgres. `migrate` = live apply + ledger (D42). `based serve` is dialect-aware — the CLI branches on the manifest dialect to build the MariaDB/Postgres/SQLite backend (D66). Packaged as a container image (`docker/Dockerfile`, D66). SQLite `foreign_keys` pragma set ON at connection setup (explicit + greppable) so opt-in `@fk` cascade/restrict enforce — cascade proven live (D108). *Open:* durable multi-instance idempotency store. |

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
  (`E0150`/`E0151`); `in` value-list elements checked per element against the column (enum
  membership `E0154`, family `E0151`, D93); param annotation vs. mapped column (`E0152`, D1).
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
- **Upsert** (mutations.md, D102): a `create … on conflict (target) update { … }` validates the
  target is a declared unique key (`E0250`), the update branch doesn't move it (`E0251`), every
  target column is set by the create or scope-managed (`E0252`), the model isn't `@soft_delete`
  (`E0253`), and a scoped model's target carries its scope column (`E0254`, cross-scope-write
  guard); the update branch is otherwise an ordinary update (E0153/E0231).
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
- `id: Id` is written in source (D103): a model declares its own `id`, or opts a keyless
  legacy table out with `@no_id("reason")` (E0261/E0262). A `@no_id` model forfeits the
  id-keyed ops — get-by-id, keyset id tiebreaker, create read-back (E0263/E0264), and can't
  be a forward-relation target (E0265).
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
- Lints: `W0100` nondeterministic `list`, `W0102` raw SQL on a `@soft_delete` model, and the declared-index
  lints (indexing.md, D15/D103, `indexes.rs`): `W0104` useless-index, `W0105` stale annotation. Index
  *requirements* are errors, not silent inference: a traversed join key (or a scanning root filter) with no
  covering `@index` is `E0260` (satisfied by `@index` or `unindexed(…)`; carries a one-key autofix), and a
  model that declares no `id` is `E0261` (autofix inserts `id: Id`). No inferred-index baseline — the DDL's
  indexes are exactly the written `@index` set (soft-delete-leading rendering preserved for a declared index).

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
