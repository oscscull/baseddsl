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
