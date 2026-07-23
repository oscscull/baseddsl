# syntax/migrations.md

Principles: 1 (destructive = explicit + visible), 2 (no silent drop/rename), 4 (schema is
the one source of truth; migrations point at it), 6 (`raw` escape — mandatory, minimal-scope,
greppable, linted), 7 (engine owns the tx boundary + the ledger; author lends intent).

Status: DRAFT for human review. Design settled in PLAN Track E; this is its prose form. Open
sub-details are flagged inline as **TODO**.

---

## The model: declarative source, versioned artifacts

The `.bsl` schema is the single source of truth (principle 4). A **migration** is the generated,
reviewable, editable derivative that carries a database from schema-state N → N+1. This is the
*versioned* model (Prisma/Atlas-versioned), **not** live declarative-apply: the tool never diffs
the running database and mutates it in place. It diffs your `.bsl` against the **last captured
snapshot** and writes a migration file you read, edit if needed, commit, and later apply.

```
edit *.bsl  ──▶  based migrate gen  ──▶  migrations/NNNN_slug/{up.mig, schema.snap[, down.mig]}
                     │
                     └─ diff( last schema.snap , current .bsl )  →  neutral step list
```

Everything the generator needs is **offline and deterministic**: the diff is `.bsl` (parsed +
checked) against a stored snapshot, no database round-trip. That is what makes the artifacts
git-diffable, CI-checkable, and the editor drift check (below) possible with no infra.

`based gen sql` (the from-scratch full DDL, D10/D28/D29) is unchanged and complementary:
`0001_init`'s `up.mig` renders to exactly that DDL. Migrations are the *incremental* path
`based gen sql` never had.

---

## Directory layout

Migrations live under `migrations/` at the project root (sibling of the `.bsl` sources), one
directory per migration:

```
migrations/
  0001_init/
    up.mig            # the neutral step list (required)
    schema.snap       # resolved schema state AFTER this migration (required)
  0002_add_product_barcode/
    up.mig
    schema.snap
    down.mig          # OPTIONAL, author-written (never generated)
```

- **`NNNN` — zero-padded, sequential, gap-free.** Four digits (`0001`…). The number is the total
  order in which migrations apply; the ledger enforces that order. Zero-padding keeps lexical
  sort == apply order in every tool (git, `ls`, the LSP).
- **`slug` — a short human label** derived from the `[name]` argument to `based migrate gen`
  (snake_cased), or auto-generated from the dominant change if omitted (`add_product_barcode`,
  `drop_user_legacy_id`). The slug is cosmetic — only `NNNN` orders — but it is what a reviewer
  reads in a directory listing, so it is not optional in the name.
- **The latest `schema.snap` is the diff baseline.** `based migrate gen` reads the highest-`NNNN`
  migration's `schema.snap`, diffs the current `.bsl` against it, and writes the next `NNNN`.

---

## `schema.snap` — the canonical snapshot

`schema.snap` is a **canonical, deterministic, dialect-neutral, human-diffable** serialization of
the *resolved* schema (`CheckedSchema`, D-sema) as of that migration. It is the baseline the next
diff runs against, so it must capture everything a structural diff can turn into a step: tables,
columns (physical type family, nullability, default, unique), relations (as their FK columns),
indexes (declared + the inferred join-key baseline, D15), soft-delete/created/updated roles, and
`@scope`. It does **not** capture queries, mutations, shapes, or filters — those emit no DDL, so
they never produce a migration step.

**Representation: a stable-ordered, indented text block in the schema's own vocabulary** (not JSON,
not SQL). Rationale:

- **Dialect-neutral** — it records `int`, `text`, `uuid`, `timestamp`, not `BIGINT`/`TIMESTAMPTZ`.
  The same snapshot drives SQLite, MariaDB, and Postgres renders (principle 4 — one truth, three
  targets). A SQL snapshot would bind it to one dialect and reintroduce drift.
- **Deterministic + stable-ordered** so a git diff shows only what changed: tables sorted by name,
  columns and indexes sorted by name within a table, every derived fact (inferred index names,
  FK column names) rendered exactly as codegen would name them. No map iteration order leaks in.
- **Human-diffable** — a reviewer reads a `schema.snap` diff in the PR and sees the schema delta
  directly, in the language they wrote, before ever reading the rendered SQL.

Serialization (finalized in E2, D39 — `based-codegen::migrate`; not in `grammar.ebnf` as it is a
generated artifact, not authored): a `snapshot v1 dialect=neutral` header, then one `table` block per
model — `table <name> [soft_delete=<col>:<mode>] [created=<col>] [updated=<col>] [scope=(…)] [sort=(…)]`,
its two-space-indented `column <name> <type> null|not_null [default=<lit>] [unique] [fk=<Model>]` and
`index <name> (<col>, …) [unique] [inferred]` lines. Tables, columns, and indexes are each name-sorted;
the default `id` (D2) is elided as an invariant. Illustrative shape:

```
# schema.snap — generated by `based migrate gen`; do not edit by hand.
snapshot v1 dialect=neutral

table order  soft_delete=deleted_at:timestamp  scope=(org_id = $ctx.org)  sort=(placed_at desc)
  column deleted_at   timestamp  null
  column org_id       uuid       not_null            # fk -> org
  column placed_by_id uuid       not_null            # fk -> user
  column status       text       not_null  default="pending"
  column total        int        not_null
  column placed_at    timestamp  not_null  default=now()
  index  idx_order_org_status  (org_id, status)
  index  idx_order_placed_at   (placed_at)
  index  inf_order_org         (deleted_at, org_id)  inferred

table product ...
```

The `id` column, being universally implicit (D2), is elided from the per-table column list and
carried as an invariant; a model that declares its own non-default `id` records it explicitly.

---

## `up.mig` — the dialect-neutral step vocabulary

`up.mig` is an **ordered list of neutral steps** in the schema's own IR vocabulary — never raw SQL
(except the explicit `raw` escape). It is what makes the two design choices above compose: the
snapshot baseline and the offline drift check both need the tool to answer "what schema do these
steps produce?" **without a database**, which is only tractable if the steps are machine-understood.
Each step renders to per-dialect SQL at `render`/`apply` time via the existing `Dialect` seam
(D21/D28/D29), so the SQL can never drift from the neutral step (principle 4).

### Step forms

Columns and types are named in the neutral vocabulary (`int`, `text`, `uuid`, `timestamp`, `date`,
`bool`, `json`); the renderer maps them per dialect (D10/D28/D29). An **opaque** column or index
(models.md `raw("…")`) is the one exception: its neutral type *is* the literal string the author
wrote (`raw("geometry(Point,4326)")`, or the canonical dialect-sorted map form), so the diff is a
plain string compare and the renderer emits it verbatim. That keeps an unmodelled column/index
inside the migration lifecycle — created, dropped, renamed, rebuilt like any other — instead of
living behind the schema's back in a raw migration where the snapshot is blind to it. A step may carry a `destructive`
marker (see the destructive policy) and, for a data-bearing change, its `raw` escape (below).

```
# table lifecycle
create table <name> { <column>… <index>… }        # full CREATE; 0001_init is all of these
drop table <name>                                   # DESTRUCTIVE

# column lifecycle
add column <table>.<col> <type> [null|not_null] [default=<lit>]
drop column <table>.<col>                            # DESTRUCTIVE
alter column <table>.<col> <change>…                 # one or more of:
    type <type>                                      # DESTRUCTIVE if narrowing (below)
    null | not_null                                  # not_null-without-default = DESTRUCTIVE
    default=<lit> | drop default

# rename (only ever emitted via the @was directive; never auto-guessed)
rename table <old> -> <new>
rename column <table>.<old> -> <new>

# indexes & uniqueness
add index <name> (<col>…) [using <method>]
add index <name> raw("…")                            # opaque index; body diffed as a string
drop index <name>
add unique <name> (<col>…)                           # DESTRUCTIVE over existing data
drop unique <name>

# relations = their FK column (D3). A new/dropped/retyped relation is an
# add/drop/alter column step on `<field>_id`; adding a required relation is a
# `not_null` add and follows the not-null-without-default destructive rule.

# escape hatch — data migrations / anything the neutral vocabulary can't express
raw(<dialect>) `<sql>`                               # NOT offline-verifiable for this step
```

Steps within a migration apply top-to-bottom under one transaction (principle 7 — the engine owns
BEGIN/COMMIT, never the step list). A migration's `schema.snap` is exactly what results from
applying its steps to the previous snapshot; E2's diff engine guarantees the two agree.

### The `raw(dialect)` escape step

Some changes are genuinely SQL: a data backfill, a `CHECK` the neutral vocabulary can't model, a
dialect-specific `USING` cast. The escape is a first-class step, mirroring `raw.md`:

```
raw(postgres) `update "order" set status = 'pending' where status is null`
```

- **`${param}` interpolation is *not* available here** — a migration takes no request args. A raw
  step is literal SQL for one dialect. (The `{table}`/`{id}` overrides of `raw.md` are a query-time
  facility; migrations have no bound row.)
- **Per-dialect.** A raw step names its dialect. To support all three targets you write the step
  once per dialect (`raw(sqlite)…`, `raw(mariadb)…`, `raw(postgres)…`); a target with no matching
  raw step for a required change fails `render`/`apply` loudly rather than silently skipping.
- **Marked "not offline-verifiable" (principle 6, never silent).** A migration carrying any `raw`
  step is flagged: the tool **cannot** compute the resulting schema state from opaque SQL without a
  SQL parser or a shadow DB (both declined for the baseline). So `schema.snap` for a raw-carrying
  migration reflects only the *neutral* steps; the raw step's structural effect (if any) must be
  declared alongside it so the snapshot stays honest — **TODO (E2/E3):** decide the annotation form
  for "this raw step also adds column X" (a paired neutral step vs. a `produces:` note). Until then,
  a raw step is treated as data-only (no structural effect) and the migration is stamped
  `verify: partial (raw)` so `based migrate verify` reports it can't fully check that migration
  offline. This is the same greppable "guarantees stop here" contract as every other raw hatch.

### Worked example — commerce: add a nullable column + an index

Suppose `Product` (spec/examples/commerce) gains a nullable barcode and an index on it:

```
Product {
  ...
  barcode: text?
  @index barcode
}
```

`based migrate gen add_product_barcode` diffs the current `.bsl` against `0001_init`'s snapshot and
writes `migrations/0002_add_product_barcode/up.mig`:

```
# migrations/0002_add_product_barcode/up.mig — generated; edit if needed, then apply.
add column product.barcode text null
add index idx_product_barcode (barcode)
```

Neither step is destructive (a nullable add, a new non-unique index), so no acknowledgement is
required to apply. `based migrate render` shows the per-dialect SQL:

**SQLite** (`text`→`TEXT`; indexes are separate `CREATE INDEX`, D28):
```sql
ALTER TABLE `product` ADD COLUMN `barcode` TEXT NULL;
CREATE INDEX `idx_product_barcode` ON `product` (`barcode`);
```

**MariaDB** (`text`→`VARCHAR(255)`, backtick quoting, D10):
```sql
ALTER TABLE `product` ADD COLUMN `barcode` VARCHAR(255) NULL;
CREATE INDEX `idx_product_barcode` ON `product` (`barcode`);
```

**Postgres** (`text`→`TEXT`, double-quote quoting, separate `CREATE INDEX`, D29):
```sql
ALTER TABLE "product" ADD COLUMN "barcode" TEXT NULL;
CREATE INDEX "idx_product_barcode" ON "product" ("barcode");
```

The neutral step is written once; three renders fall out of the `Dialect` seam (the
column's `NULL`/`NOT NULL` is stated explicitly in all three — valid on each, verified
against real servers). An `alter column` step diverges more sharply, because the dialects
differ on in-place column change: **Postgres** emits one `ALTER COLUMN … TYPE/SET NOT
NULL/DROP NOT NULL/SET DEFAULT/DROP DEFAULT` per change; **MariaDB** restates the whole
column via `MODIFY COLUMN` (it has no piecemeal form); **SQLite** has *no* in-place
`ALTER COLUMN` at all, so the renderer emits a loud comment pointing at a hand-authored
`raw(sqlite)` table-rebuild rather than broken SQL (principle 6 — the escape is never
silent). `0002`'s
`schema.snap` is `0001`'s snapshot plus the `barcode` column and `idx_product_barcode` index.

---

## `@was("old_name")` — the rename directive

A rename is invisible to a structural diff: dropping `old` and adding `new` is
observationally identical to renaming, and guessing wrong destroys data. So **the default for a
column/model whose name changed is drop + add** (safe, visible, principle 2 — never auto-guessed).
To get a clean `RENAME` instead, you *declare* the rename in `.bsl` with `@was`:

```
Product {
  ...
  barcode: text? @was("upc")      # field-level: column upc -> barcode
}

@was("legacy_product")            # model-level: table legacy_product -> product
Product { ... }
```

- **Field-level** `@was("old_col")` sits in the field's modifier position; it names the *previous
  physical column name*. The diff, seeing `barcode` absent from the snapshot but `@was("upc")`
  present and `upc` present in the snapshot, emits `rename column product.upc -> barcode` instead of
  a drop+add pair.
- **Model-level** `@was("old_table")` is a decorator; it drives `rename table`.
- **`@was` is a diff-time directive, transient by nature — and `gen` retires it for you.** Once
  `based migrate gen` writes a migration that *consumes* a `@was` (its `rename` step is emitted), it
  **self-consumes the spent directive**: it strips that exact `@was("old")` token from the `.bsl`
  source and prints a visible line (`removed spent @was("old") from Model.field (rename captured in
  migrations/NNNN_slug/)`). The rename now lives durably in the ledger — the `rename` step in `up.mig`
  and the new name in `schema.snap` (principle 4, one source of truth) — so the source hint is dead
  weight the moment it is captured. The removal is surgical: only the directive (and its adjacent
  separator, or its whole line for a model-level decorator that sits alone on one line) is removed, so
  the rest of the declaration's formatting is untouched. The rewrite is conservative — **only** a
  `@was` whose `rename` step was actually emitted is removed, so a spent or still-live `@was` (which
  produces no step) is never silently edited. For the case where `gen` didn't run (a hand-authored
  migration), the LSP still flags a lingering spent `@was` as **`W0107`** "rename already captured —
  remove it" (the fallback).
- **Teach-at-checkpoint — `@was` reveals itself at the ambiguous moment.** A rename authored *without*
  `@was` is, to a structural diff, a drop of X plus an add of Y — indistinguishable from a genuine
  drop+add, and applied as a destructive drop. So when a single-table diff **drops one column X and
  adds one same-family column Y**, the tool prints a hint — `if this renames X → Y, add @was("X") on Y
  and re-run based migrate gen; otherwise X is dropped (data loss)` — in three places: `based migrate
  gen` stdout, the offline `W0108` drift note, and the `based migrate apply` destructive gate. This
  gives `@was` a self-revealing property over the run→read→edit→re-run loop with zero prior knowledge,
  and needs no interactive prompt (which would hang a non-TTY harness and make `gen` non-reproducible).
- **No cross-inference.** `@was` maps exactly one old name to one new name. The tool never chains or
  guesses a rename from a type/position match; a rename you don't declare is a drop+add, full stop.
  (The teach hint *suggests* `@was` at the one-drop/one-add moment; it never applies one — a two-drop
  or two-add diff is left silent precisely because the pairing would be a guess.)

`@was` is the *only* new authored `.bsl` surface migrations introduce — the migrations themselves
are generated artifacts, not written by hand. It is added to `grammar.ebnf` as a `modifier`
(field-level) and a decorator (model-level).

---

## Destructive-change policy (principle 1)

A change that can lose or reject data is **generated** (never omitted — you must see it) but is
**marked destructive and refuses to apply without an explicit acknowledgement**. Destructive
changes:

| Change | Why destructive |
|--------|-----------------|
| `drop table` | deletes all rows |
| `drop column` | deletes a column's data |
| `alter column … type <narrowing>` | may truncate/fail (e.g. `text`→`int`, widening `int`→`text` is safe) |
| `alter column … not_null` **without** a `default` | existing NULL rows violate the constraint |
| `add unique` over an existing table | existing duplicate rows violate it |

A safe change (nullable add, widening, new non-unique index, `drop`ping a constraint, a rename via
`@was`) applies with no ceremony.

**Two acknowledgement forms, both loud + greppable:**

- **`--allow-destructive`** on `based migrate apply` — the operator, at apply time, vouches for this
  run. Without it, `apply` stops before the first destructive step and lists them.
- **`unsafe("reason")`** written into the `up.mig` step — the *author*, at review time, annotates a
  specific destructive step with a mandatory reason. A step carrying `unsafe("…")` is pre-vouched:
  it applies without the CLI flag, and the reason is greppable in the migration + shown in `status`.
  This is the `unindexed(unsafe, "reason")` pattern (indexing.md) applied to migrations: "guarantees
  end here, a human vouched, here's why."

```
drop column product.legacy_sku  unsafe("replaced by sku in 0003; backfilled there")
```

Neither form is silent (principle 6). A destructive migration with neither acknowledgement is a
hard `apply` error, never a quiet data loss.

---

## Rollback: roll-forward by default

**The default rollback strategy is roll-forward** — you fix state by writing the *next* migration,
not by reversing the last. Reason: an auto-generated "down" is a fiction (dropping a column can't
restore its data; reversing a backfill needs the pre-image), and a fiction that looks like a safety
net is the worst quadrant of principle 1.

- **No `down.mig` is ever auto-generated.** Silence here is honest: the tool will not pretend a
  reverse exists.
- **An OPTIONAL author-written `down.mig` is honored if present.** When you *can* write a correct
  reverse (a pure additive migration, a reversible rename), you author `down.mig` by hand as **raw
  per-dialect SQL** (`;`-terminated statements — the honest form for a hand-written reverse, D42): a
  neutral-vocabulary down would need a lossless neutral-step *text parser* the engine deliberately
  doesn't have (the up path is snapshot-authoritative, not text-parsed — E3/E4), and someone writing a
  reverse is writing SQL anyway (this mirrors the `raw(dialect)` escape). Its absence means "this
  migration is roll-forward only," which is stated in `status`, not hidden.
- **Down-invocation surface (resolved E4).** `based migrate apply --down` rolls back the single most-
  recently-applied migration; `based migrate apply --to <NNNN>` reconciles the applied set to exactly
  `{≤ NNNN}` — rolling *forward* pending migrations up to `NNNN`, or rolling *back* (newest first, each
  via its `down.mig`) anything applied above it (`--to 0` rolls back everything). Each rollback runs in
  a transaction that also deletes the migration's ledger row. A rollback of a migration with no
  `down.mig` is a hard error, never a silent skip.

---

## The `_based_migrations` ledger

The engine records which migrations have applied, in a `_based_migrations` table it owns and creates
on first `apply` (principle 7 — the engine owns the ledger; the author never writes it):

```
_based_migrations
  id           text        not null primary key   # the NNNN_slug directory name
  content_hash text        not null               # hash of up.mig's canonical bytes
  applied_at   timestamp   not null               # when apply committed
```

- **One row per applied migration**, inserted inside that migration's own transaction — so a
  half-applied migration (crash mid-apply) leaves no ledger row and re-`apply` retries it cleanly.
- **`content_hash` = a stable hash of the canonical `up.mig`** (resolved E4,
  `based_codegen::migrate::content_hash`): canonicalization drops comment (`#…`) and blank lines and
  trims each remaining line (so a cosmetic whitespace/comment edit doesn't trip the guard, but any
  change to a step does), then FNV-1a-64 over those bytes, rendered as 16 lowercase hex digits — the
  same FNV family the runtime uses for request fingerprints (D31); collision resistance is not
  security-critical (this guards an accidental post-apply edit, not an adversary), so a fast
  non-cryptographic hash is the right tool. This is the tamper/drift guard.
- **Tamper rule (loud, principle 1):** at `apply`/`status`/`verify`, the ledger's stored hash for an
  already-applied migration is compared to the current file's hash. **A mismatch — the `up.mig` was
  edited after it was applied — is a hard error**, never a silent re-apply. An applied migration is
  immutable history; if it was wrong, you fix forward with a new migration. (Editing a *not-yet-
  applied* migration is fine and expected — that's the review loop.)

Applied migrations run **in `NNNN` order**; a gap or an out-of-order pending migration is a `status`
error (the total order is the ledger's invariant).

---

## Offline LSP drift diagnostic

Because the diff baseline is a stored snapshot (not a DB), the editor can answer **"is my `.bsl`
ahead of my migrations?"** with no infrastructure — the same `based-facts`/diagnostics path the
rest of the LSP uses (M5), fully offline:

> **N uncaptured schema changes — run `based migrate gen`.**

Computed as `diff(latest schema.snap, current .bsl)`: if that diff is non-empty, the schema has
structural changes not yet captured in a migration. The diagnostic anchors at the changed
declaration(s) and lists them (an added column, a dropped model), so a reviewer sees exactly what a
`based migrate gen` would capture *before* running it. An empty diff = the migrations are up to date,
no diagnostic. This is a **CLI + editor** concern only; **live-DB drift** (has someone changed the
running database out from under the migrations?) stays a CLI-only, connect-required check, out of the
offline LSP path.

---

## Command surface

`based migrate <subcommand>` — all offline except `apply`/`status`/`verify` against a live DB.

| Command | What it does |
|---------|--------------|
| `based migrate gen [name]` | Diff current `.bsl` vs. the latest `schema.snap`; write the next `migrations/NNNN_slug/{up.mig, schema.snap}`. No-op (exits clean, writes nothing) if the diff is empty. Offline. |
| `based migrate render [--number NNNN] [--dialect D]` | Render migrations' steps to per-dialect SQL and print it — the review-the-SQL step (E3, done). `--number` picks one migration, else all in order; `--dialect` overrides the manifest target. Offline: re-derives each migration's steps as `diff(snapshot[N-1], snapshot[N])` from the stored `schema.snap`s (the snapshot-authoritative model, which `verify` asserts equals the `up.mig`), so no `up.mig` parser is needed yet. Does not touch a DB. |
| `based migrate apply [--allow-destructive] [--to NNNN] [--down]` | Apply pending migrations in order, each in one transaction, inserting the ledger row. Checks the tamper hash first; gates destructive steps on the ack. Honors a `down.mig` for `--down`. Live DB. |
| `based migrate status` | Show applied vs. pending migrations, flag any hash mismatch, any gap/out-of-order, and (if a DB is reachable) the ledger state. |
| `based migrate verify` | Offline: confirm each `schema.snap` equals its predecessor + its `up.mig` steps applied, and that the latest snapshot matches the current `.bsl` (no uncaptured drift). Reports raw-carrying migrations as `partial`. The CI gate. |

---

## Deferred / out of scope for v1

- **Live-DB schema drift detection** (introspect the running DB, compare to the expected snapshot):
  a CLI-only, connect-required feature. The offline snapshot-vs-`.bsl` drift (above) is the v1 story;
  live drift is a later `based migrate status --live` (TODO).
- **Multi-instance apply coordination** (two deployers racing `apply`): the ledger's per-migration
  transaction gives single-writer safety, but an advisory-lock/leader story for concurrent deployers
  is deferred (parallels D25's durable-idempotency-store deferral).
- **Down-migration auto-generation** — deliberately never built (see Rollback).
- **Raw-step structural effect declaration** — see the `raw(dialect)` TODO; until pinned, raw steps
  are data-only and their migration is `verify: partial`.
- **Snapshot format as authored grammar** — `schema.snap` is a generated artifact; it gets a grammar
  entry only if it ever becomes hand-editable (not planned).
