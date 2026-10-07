# Recovering a failed migration

Stop migration writers and application writes, take a backup of the database and
migration files, and inspect every affected shard before retrying. An error names
the migration, up/down direction and statement position (one-based, in
`based migrate render` execution order), or the ledger/commit stage. Previously
completed migrations stay applied. A failed commit can have an ambiguous outcome.

MariaDB/MySQL DDL implicitly commits: a later statement can fail after earlier
schema changes persist, with no completion row in `_based_migrations`. A pending
status is therefore **not proof that no SQL ran**. Blind retry can fail on existing
objects. There is no automatic repair, statement resume, or rollback guarantee.
Postgres and SQLite ordinary DDL is transactional: a statement failure rolls back
the migration's DDL and ledger update. Authored raw SQL must respect that contract;
SQL that ends transactions or performs nontransactional effects changes it.

## Tested recovery fixture

The `based-cli` test `migration_failure` applies `0001_init`, creating
`widget` with its generated ID and `name TEXT NOT NULL`, and seeds `name = 'keep me'`. Migration
`0002_add_size` adds a nullable integer `size`, then its raw SQL attempts to add
`name` again. Statement 2 fails. MariaDB retains `size`; Postgres/SQLite do not.
Only `0001_init` appears in the ledger.

In a disposable database, reproduce and inspect the fixture:

```sh
TEST_MARIADB_URL=mysql://root:based_test_pw@127.0.0.1:13306/based_test \
  cargo test -p based-cli --test migration_failure mariadb_ -- --nocapture
```

The test resets `widget` and the ledger: never point it at application data.
The manual recovery it verifies is:

1. Inspect `SHOW CREATE TABLE widget`, `SELECT * FROM widget`, and
   `SELECT * FROM _based_migrations ORDER BY id`. Check that only `0001_init` is
   complete and the newly added `size` column is unused. Back up before repair.
2. On MariaDB, restore the pre-migration schema with
   `ALTER TABLE widget DROP COLUMN size`. This fixture has no data in that new
   column. Never use this repair if the column holds real data; retain/export it
   and design a reviewed repair instead. Postgres/SQLite need no schema repair.
3. Remove the invalid duplicate-column raw step from the **never-completed**
   `0002_add_size/up.mig`. Keep its snapshot and valid add-column step. Check all
   shards first: never edit a migration already recorded as applied anywhere.
4. Render and review, then run `based migrate apply`. Verify `name = 'keep me'`,
   `size IS NULL`, and both completion rows. A second apply is a no-op.

Other failures require their own reviewed recovery, or restoration from backup.
Do not fabricate a ledger row to hide an incomplete schema. A failed down migration
also requires inspection: MariaDB can retain reverse DDL while its ledger row remains.

## Data and history boundaries

Use `@was` for a rename that must preserve values; drop/add is destructive and
can lose data. `--allow-destructive` acknowledges risk, it does not create a backup
or make changes reversible. An absent or comment-only `down.mig` is roll-forward
only; a down script cannot restore dropped values without an external backup.
Test reverse scripts on a copy before relying on them.

`based migrate verify` compares BSL, stored snapshots and structural migration
artifacts offline. `based migrate status` compares stored migration history and
ledger hashes. Neither detects live database drift or proves a pending migration
left the database untouched. Applied history is immutable; use a new migration
for changes to completed history. Check live schema and data separately.
