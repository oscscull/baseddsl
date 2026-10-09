# Importing an existing database

Import reads explicitly selected catalog metadata and writes checked BSL models.
It leaves data and migration ownership with your existing system. It does not
baseline migrations, apply DDL, infer permissions, or generate an application.

In a configured project with an empty `models/` directory:

```toml
dialect = "sqlite"
root = "models"
```

```sh
based import --database-url original.db --table main.account --table main.entry --json
```

For PostgreSQL, select `schema.table`; for MariaDB, `database.table`.
Select related targets too. Imports refuse existing output files and symlinks;
keep imported models as hand-owned source and review the report before using them.
Add your own shapes, queries, scopes, and lifecycle policy, then generate a client
and connect it to the original database without `migrate apply`.

## Connections and supported metadata

SQLite opens an existing file read-only and can see committed WAL schema changes.
It does not create a missing file. PostgreSQL reads catalogs in one read-only,
repeatable-read snapshot. MariaDB needs complete metadata visibility; use the
REFERENCES-only fixture pattern for metadata access, rather than application SELECT
privileges. Concurrent MariaDB DDL can invalidate discovery; retry after schema
writers stop. Preserve verified TLS settings on server URLs.

The report retains native types, defaults, generation, ordered keys/indexes, and
foreign-key facts. Emission preserves physical names with aliases and validates
mapped columns and relationships against those facts. Required natural/composite
keys and keyless tables remain distinct. Naming collisions are resolved deterministically.

Unsupported or unrepresentable facts block publication rather than disappearing.
Review findings for views, expression/partial indexes, generated expressions,
native types/defaults, and FK semantics. Signed native defaults and arbitrary
composite-FK column aliases currently have representation limits. Selected objects
with missing visibility or unresolved references cannot become successful imports.

Import is a one-shot starting point, not continuous schema synchronization. To
adopt Based migrations later, choose an explicit migration ownership transition;
import does not perform one for you.
