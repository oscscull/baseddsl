# syntax/raw.md

Principles: 6 (never silent), 1.

## Rule: raw at the leaves, never the structure
Raw goes where the engine needs only a value. Forbidden where it needs meaning (can't inject soft-delete into joins it didn't build).

## Levels (smallest scope first)
- Raw value in a shape: `full_name = raw`concat(first,' ',last)``
- Raw predicate term: composes as one boolean leaf with `where`; engine still wraps soft-delete around it.
- Raw whole query / raw join: full trapdoor. You own soft-delete predicates + dialect portability. Engine still gives param-binding + result-typing.

## Guarantees through the hatch
- `${input}` always interpolates as a bound parameter, never string concat.
- All raw marked with the `raw` backtick form = greppable inventory of where guarantees stop.
- Engine detects raw touching a `@soft_delete` table and lints the gap ("can't verify tombstone filter — confirm"). Never silent.

## Whole-query raw
The query's block body is one `raw` backtick block — the block IS the statement:
```
query heavy_users(min: int) -> UserRow[] {
  raw`SELECT u.name AS name, u.email AS email
      FROM {table} u JOIN order_item oi ON oi.user_id = u.id
      WHERE u.total >= ${min} AND u.deleted_at IS NULL`;
}
```
What survives the hatch: `${param}` binds positionally (never concat); `{table}`/`{id}`
interpolate the target model's table/key; the declared shape types the result columns
**by name** (the engine trusts the SQL to produce them — a missing column is a decode
error at the caller, not a silent null); the generated client/OpenAPI surface is
identical to an engine-built query of the same signature. What you own: soft-delete
predicates (`u.deleted_at IS NULL` above — linted W0102 on the target model and on any
soft-delete table the SQL mentions, never injected), scope filters, ordering, and
dialect portability.

Composition is enforced, not implied: params must carry explicit types and no binding
(no column to infer from, no engine-built WHERE — E0210); `${ctx.…}` has no type
source (E0214 — pass a typed param); a scoped target requires `unscoped("reason")`
(`scoped` would promise an injection that can't happen, E0211); `-> stream` is
rejected (E0212); the return shape must be flat (nests need engine-built projections,
E0213). No sort cascade, no `page`, no index lint — the SQL owns all of it.
