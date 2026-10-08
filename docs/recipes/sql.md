# Inspect generated parameterized SQL

Task: review what a modeled query compiles to before changing a database. In
either initialized tutorial project:

```sh
based check
based gen sql
based gen sql --check
```

Read the configured `generated/schema.sql`: it contains DDL and named
parameterized query/mutation templates. Inspect selected columns, relation
projection, scope predicates, placeholders, and dialect-specific lowering.
Generation does not execute this SQL or apply migrations. Templates do not
include actual host context values and are not query-plan/performance evidence.
Database-specific plan inspection needs the real database and representative
bind values, with that host's access controls.

The [artifact guide](../generated-artifacts.md) owns output paths, freshness,
and overwrite behavior. For a migration inspect its rendered SQL with
`based migrate render . --number NUMBER` before explicit application, as in the
[embedded rename](../embedded-tutorial.md#rename-a-field-without-losing-data).
Use the [reference](../reference.md) for syntax and
[measured runtime workloads](../runtime-performance.md) for performance evidence.
