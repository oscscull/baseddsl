# syntax/raw.md

Principles: 6 (never silent), 1.

## Rule: raw at the leaves, never the structure
Raw goes where the engine needs only a value. Forbidden where it needs meaning (can't inject soft-delete into joins it didn't build).

## Levels (smallest scope first)
- Raw value in a shape: `full_name = sql`concat(first,' ',last)``
- Raw predicate term: composes as one boolean leaf with `where`; engine still wraps soft-delete around it.
- Raw whole query / raw join: full trapdoor. You own soft-delete predicates + dialect portability. Engine still gives param-binding + result-typing.

## Guarantees through the hatch
- `${input}` always interpolates as a bound parameter, never string concat.
- All raw marked with the sql backtick form = greppable inventory of where guarantees stop.
- Engine detects raw touching a `@soft_delete` table and lints the gap ("can't verify tombstone filter — confirm"). Never silent.

Example (raw whole query): a `sql` backtick block selecting users joined to order_items, where you write `u.deleted_at is null` yourself because the engine won't inject it.
