# Migrations and recovery

Migration planning is an offline diff of BSL against the previous snapshot:

```sh
based migrate gen
based migrate verify
based migrate render . --number 1
based migrate apply --database-url local.db
based migrate status --database-url local.db
```

Review the rendered SQL before applying it. Use `@was("old_name")` for renames;
drop/add can lose values. Applied migration files are immutable. Add another
migration to change an applied schema.

## Data steps

Write backfills and reference rows as typed lines in `up.mig`, between the
structural steps. They name the snapshot's tables and columns as of their line:

```
format 2
add column card_printing.set_code text null
update all card_printing { set_code = data.set }
update card where (card_printing.any(digital = false)) { paper = true } chunk 5000
create format[] from [
  { key = "commander", name = "Commander" },
] on conflict (key) update { ...incoming }
delete event where (created_at < "2024-01-01")
```

`format 2` runs every line in written order, `raw(<dialect>)` lines included.
Without it, a migration keeps the old order: all `raw` lines after the
structure. `based migrate gen --data` writes a migration with no schema change
to hold data steps.

`verify`, `render`, and `apply` check each step: unknown names, type mismatches,
generated columns, missing required columns, and a non-unique conflict target
are errors, as is anything the target dialect can't run. `data.set` reads a key
of a `json` column. `child.any(…)` tests for a row in the table whose foreign
key points here; when it has several, name one: `deck.any(by partner_id, …)`.
Enum values are written bare in their stored spelling (`status = OPEN`). A
`create` that omits the primary key gets an id minted at apply time with the
project's id strategy (uuid or ulid).

`chunk N [by col] [pause 200ms]` on an `update` or `delete` walks the primary
key (or `col`, which must be unique or indexed) `N` rows at a time, one
transaction per batch. Progress is recorded, so an interrupted apply resumes.

`--down` reverses a migration of literal-row `create`s by itself. Any other
data step needs a `down.mig`. `migrate gen` writes it as a `format 2` script of
`raw(<dialect>)` reverse lines, and data steps can sit between them; they are
checked against the state the up migration leaves.

## Recovery

A failed MariaDB DDL statement can leave earlier changes committed with no
completion ledger row. Pending status does not prove that nothing ran. PostgreSQL
and SQLite ordinary DDL rolls back on statement failure; raw SQL that changes
transaction behavior can break that assumption. Commit failures can be ambiguous.

After a failure, stop writers, back up data and migration files, and inspect the
live schema and `_based_migrations` on every shard. Restore a known state or
prepare a reviewed repair before retrying. Never fabricate a completion row or
edit history already applied on another shard.

`migrate verify` checks stored artifacts offline; `migrate status` checks
ledger history. Neither detects arbitrary live schema drift. Reverse migrations
cannot restore dropped data without a backup; `--allow-destructive` is an
acknowledgment, not a recovery mechanism.
