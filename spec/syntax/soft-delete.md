# syntax/soft-delete.md

Principles: 1, 2 (tombstone is a real field), 5, 6.

## Decorator points at a real field
Tombstone is a normal, visible field. Decorator marks its role.
```
@soft_delete(deleted_at)
Order {
  deleted_at: timestamp?
  ...
}
```

## Three operations
Engine generates read-filter, delete action, restore action — and rewrites them through the system.

## Covered subset (auto from type)
Type determines all three:
- nullable `timestamp`/`date`: live `IS NULL`, delete `= now()`, restore `= NULL`
- `bool`: live `= false`, delete `= true`, restore `= false`

Outside this subset = error, directing to raw. Engine refuses to guess. (Boundary = "does the type determine the operations?")

## Per-operation override
Replace one operation (commonly `restore`) with raw, keep others auto. Raw override gets safe interpolation `{table}`, `{id}`:
```
@soft_delete(deleted_at)
Order {
  deleted_at: timestamp?
  restore: sql`update {table} set deleted_at = null, status = 'active' where {id}`
}
```

## Rewriting
`delete` on a soft-delete model is never real SQL `DELETE` — rewritten to the declared action. Real delete requires explicit loud `hard delete`.

## Injection guarantee
Soft-delete predicate injected into every generated query: across joins, aggregates, and pagination page-counts (filter before limit; page size counts live rows). Headline guarantee; works because it's a compiler primitive.
