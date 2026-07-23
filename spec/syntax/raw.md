# syntax/raw.md

Principles: 6 (never silent), 1.

## Rule: raw at the leaves, never the structure
Raw goes where the engine needs only a value. Forbidden where it needs meaning (can't inject soft-delete into joins it didn't build).

## Levels (smallest scope first)
- Raw value in a shape: `full_name = raw`concat(first,' ',last)``
- Raw predicate term: composes as one boolean leaf with `where`; engine still wraps soft-delete around it.
- Raw whole query / raw join: full trapdoor. You own soft-delete predicates + dialect portability. Engine still gives param-binding + result-typing.

## The one spelling: `raw`
`raw` is the *only* escape-hatch keyword. `sql` is never a keyword, marker, or decorator
anywhere in the language — we are Postgres-compatible, so `sql` reads ambiguously. Two
positions share the word, distinguished by their delimiter:

| form | position | what it holds |
|------|----------|---------------|
| ``raw`…` `` (backticks) | value / predicate / whole query / soft-delete override / `.mig` step | SQL text, with `${param}` bound and `{table}`/`{id}` interpolated |
| `raw("…")` (parens) | a model field's **type**, a model's **`@index`** | an opaque literal string the engine stores and diffs but never interprets |

## Opaque types + indexes — `raw("…")`
The parenthesized form is the structural escape hatch: it lets one column or one index leave
the engine's vocabulary without the whole model leaving the system.

```
location: raw("geometry(Point,4326)")?
tags:     raw({ postgres: "tsvector", mariadb: "text" })?

@index location using gist
@index raw("(lower(email))")
```

The literal is carried verbatim into the DDL **and the neutral snapshot**, so the migration
diff is a plain string compare: an opaque column/index is created, dropped, renamed, and
rebuilt like any other, and the drift check still sees the whole table. The alternative — a
raw migration adding the column behind the schema's back — makes the snapshot blind on a
modeled table, loses the column to any sqlite table rebuild, and drops it from every generated
surface.

What the hatch forfeits, loudly: the value is opaque, so writing it is `E0273` (make it
nullable or defaulted) and filtering/sorting/aggregating it is `E0271` — read it through the
raw *value* leaf instead (`area = raw`ST_Area(location)``). Everything else on the model —
CRUD, migrations, scope, soft-delete, the drift check — is unaffected. Details: models.md
(Types), indexing.md (Exotic indexes).

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
