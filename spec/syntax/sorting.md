# syntax/sorting.md

Principles: 1 (default + explicit override), 2 (unordered = consequential), 4.

Sort is a property of rows (model/relation), NOT of projection. Shapes carry no sort.

## Precedence (most-specific wins)
query `order (...)`  >  relation `@sort`  >  model `@sort`  >  none -> lint

## Model default
Absent any other instruction, the entity lists in this order. Closes the bare-form gap (bare queries have nowhere to write `order`, so the default must live on the data model).
```
@sort(created_at desc)
Post { ... }
```

## Relation default (overrides model, for that traversal)
"This entity when reached this way" may sort differently than globally. A field-level
`@sort` is valid **only on a to-many relation field** — it orders that relation's nested
collection. Placed on a scalar or a to-one relation (or inside the model body expecting to
set the model default — that spelling goes *before* the model) it orders nothing, so it is
rejected (`E0348`) rather than silently dropped.
```
User {
  posts: Post[] (Post.author) @sort(pinned desc, created_at desc)
}
```

## To-many nests
A shape's to-many nest (`items { ... }`) is a traversal, so its array follows the same
cascade minus the query tier: relation `@sort` > target model `@sort` > unspecified.
Query `order` never reaches inside a nest — it orders the query's own rows. (Shapes still
carry no sort; the order is a property of the traversed rows, declared on the data model.)

The nest's order lowers to an `ORDER BY` *inside* the JSON aggregate. MariaDB, Postgres,
and SQLite (≥ 3.44) all honor an ordered aggregate; **MySQL's `JSON_ARRAYAGG` has no
`ORDER BY` clause** (it is a syntax error, not a silent no-op). So an ordered to-many nest
(or m2m flatten) targeting MySQL cannot be generated and is rejected at `check` (`E0350`) —
target MariaDB, or leave the traversal unordered. An *unordered* nest works on every target.

## Query override (most specific)
```
query recent(user -> author) -> PostShape[] order (updated_at desc);
```

## NULL placement (`nulls first` / `nulls last`)
Where NULLs fall relative to non-NULL values, per sort key. Without it, the placement is the
**dialect default** — and that default differs (Postgres: nulls last on `asc`; MariaDB/MySQL/
SQLite: nulls first) — so a portable query needs the modifier to pin it.
```
query q() -> Card[] order (assignee asc nulls last);
User { posts: Post[] (Post.author) @sort(pinned desc, edited_at desc nulls last) }
```
Valid in query `order` and in `@sort`, after the direction. Lowered portably: native
`NULLS FIRST|LAST` on Postgres/SQLite; a leading `col IS NULL` term on MariaDB/MySQL (which have
no such clause — `col IS NULL` is 0 for non-NULL, 1 for NULL, so `asc` trails NULLs). Meaningful
only on a nullable key (a no-op elsewhere). Keyset pagination's cursor follows the chosen
placement, so a paginated `nulls last` list stays consistent across pages.

## Lint
A `list` with no sort at any tier returns nondeterministic order -> warn ("results nondeterministic; add @sort or order"). Same cheap-lint-prevents-prod-surprise pattern as `unindexed`. (Keyset pagination already forces a sort; this mainly catches non-paginated lists.)
