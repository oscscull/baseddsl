# decisions.md

Implementation decisions the prose specs left open. Each resolves an ambiguity the
grammar and compiler must commit to. Governed by `principles.md`; where a default is
chosen it follows principle 2 (omission must have one safe meaning).

Status: proposed. These are my reads of the spec — flag any you'd decide differently.

## D1 — `Id` type, default PK = uuid
`Id` is a primitive scalar: the opaque primary-key type. The concrete column type of the
implicit `id` is **`uuid` by default** (distributed-friendly, non-enumerable; MariaDB native
`UUID` where available, else `BINARY(16)`).
- A model whose key is something else declares it explicitly (deviation visible, principle 2).
- `Id` is the type of the implicit `id` column and of any relation's foreign-key value.
- In query params, `org: Id` means "the key of the referenced row." Same-name binding
  to a relation field (e.g. `org`) compares against that relation's FK column.

## D2 — `id` implicit; timestamps are decorated, never implicit
Only **`id: Id`** is implicit (single safe meaning; principle 2 holds). Created/updated
timestamps are NOT implicit — making them so would force `updated_at` onto tables that don't
want it, violating principle 2. They are a real declared field plus a decorator that marks the
engine-managed role, exactly parallel to `@soft_delete` pointing at `deleted_at`:
```
@created(created_at)        # set on insert
@updated(updated_at)        # set on insert + every update
product {
  created_at: timestamp
  updated_at: timestamp
  ...
}
```
A query may only sort by `created_at` if that model declares it (see the product example).
`deleted_at` likewise is a real declared field marked by `@soft_delete` (soft-delete.md).

## D3 — Table & column naming: verbatim, never pluralized
- Table name = `snake_case(ModelName)`, no pluralization. `Order` -> `order`,
  `OrderItem` -> `order_item`. (snake_case only — capitalized SQL table names break across
  case-sensitive/insensitive platforms. No pluralization — irregular plurals are a footgun and
  a magic dictionary violates principle 4.)
- Column name = the field name verbatim.
- Relation field FK column = `<field>_id`. `placed_by: User` -> column `placed_by_id`.
- Legacy override hooks (the base state we're targeting may use any names, including reserved
  words — D8): `@table("legacy_name")` on the model; `(column "legacy_name")` on a field;
  `(on: ...)` on a relation (relations.md). The convention is the default only, never a
  requirement on the existing database.

## D4 — `$ctx`
`$ctx` is a reserved param namespace holding caller-supplied request context (auth.md
Handles 1 & 2). `$ctx.org` is a path into it.
- For the parser: `$ctx` is a normal `$`-param whose name is `ctx`, followed by a dotted path.
- Its shape/type is declared in a project manifest (D5), not inferred. Typing `$ctx.*` is a
  sema concern; deferred past the parser milestone.

## D5 — Project manifest & schema discovery
A project root holds a manifest `based.toml` (name TBD) declaring:
- a format/schema version (room for migration as the language evolves),
- the dialect compile target (default `mariadb`),
- the schema source root (default the project root),
- the generated-client target language (`rust` for now),
- the `$ctx` type binding.
The manifest globs `**/*.bsl` under the schema root into the schema = the closed set of
declarations. Closed-world is required by calling.md (index inference, N+1 lint, etc.).

## D6 — One extension, uniform grammar
Single file extension `.bsl`. The grammar is uniform: any declaration (model, shape, query,
mutation, named filter) may appear in any file (grammar.ebnf `file = { decl }`). No
per-kind grammar, no "one model per file" rule. Concatenating every `.bsl` file yields the
same schema as any other partition of the same declarations.

(Extension `.bsl` is provisional; it is short and not in meaningful real-world use. Swapping
it is a one-line change in the manifest/discovery glob.)

## D9 — Free layout; recommended convention
Directory structure is the user's choice — **not enforced**. The compiler only globs `*.bsl`.

Recommended convention (what `spec/examples/commerce` demonstrates): one directory per domain
entity, with the schema and its read contracts together and the access layer separate:
```
order/
  model.bsl       # the `order` model + shapes `from Order`
  queries.bsl     # queries / mutations / filters for Order
```
A team may instead split shapes into `shapes.bsl`, put everything in one file, or group by
feature — all parse identically. The convention is guidance, never a constraint.

## D7 — Identifier casing (load-bearing, from models.md). No fold.
Casing distinguishes models from columns at **every** position — declaration and reference
alike — so the reader and lexer classify an identifier by its case alone:
- A **model** is `UpperCamel` everywhere: declared `Order { }`, referenced `Order`. Declaration
  name == reference name (no fold). `OrderItem` is declared and referenced identically.
- A **column / field / primitive** is `lower_snake`: `placed_by`, `created_at`, `text`.
- Rule (models.md): capitalized => model/relation; lowercase => column/primitive.
- The only `UpperCamel -> snake` transform is **table-name generation** (D3), done at codegen,
  overridable via `@table`. Source identifiers never need folding to relate decl and ref.

## D8 — Keywords are contextual (positional), not globally reserved
Keywords are recognized **positionally**: an identifier is read as a keyword only where the
grammar expects one, and may otherwise be used as an ordinary identifier. The set of words
that are keyword-in-position:
`get list create update delete restore hard tx where order page offset count with guard
from shape query mutation filter unindexed unsafe on column table by has in not and or
true false null now`.

Why not global reservation: the canonical `OrderItem` model names a field `order:` (a would-be
reserved word), and the same word is the `order (...)` clause keyword — so the two coexist in
one schema. This is safe because the positional grammar is unambiguous at every decision point:
a model member always starts with `ident :` (field / soft-override) or `@` (index), so `order:`
can only be a field; a query clause is `where`/`order`/`page` followed by `(`, so `order (` can
only be a clause; a statement verb is `get`/`list` at the head of a block. The lexer emits one
`LowerIdent` token for every lowercase word; the parser matches keyword text only in-position.

Tradeoff accepted: a keyword misused where an identifier is expected fails a token or two later
with a generic message rather than "X is reserved." The collision surface is small and the
grammar is tight, so this is worth the friendliness to legacy schemas.

Legacy names are still aliasable (never required): a column literally named with a keyword can be
reached as-is where an identifier is expected, or — if you'd rather not spell the keyword — via
`(column "order")` / `@table("order")` (D3). The alias is greppable and lives in one place
(principle 4); it is now a convenience, not a requirement for adopting a legacy schema.

## D10 — SQL type mapping (MariaDB DDL codegen)
The physical SQL type each primitive lowers to (`based gen sql`, dialect `mariadb`). Chosen for
correctness-by-default over a literal name match — an unbounded/overflow-prone default is a silent
footgun, and this is a DB-first tool:
- `text` -> **`VARCHAR(255)`**. Bounded so the column is directly index/unique-able (MariaDB cannot
  index a `TEXT` column without a prefix length). The convention default; length tuning is a later
  concern (no length primitive exists yet).
- `int` -> **`BIGINT`**. The single `int` primitive must not silently overflow on the common
  money/count use (prices, totals in the commerce example).
- `bool` -> **`BOOLEAN`** (MariaDB alias for `TINYINT(1)`).
- `timestamp` -> **`DATETIME`** (not `TIMESTAMP`): dodges MariaDB's implicit `ON UPDATE
  CURRENT_TIMESTAMP` on the first timestamp column and the 2038 range cap. `@created`/`@updated` are
  engine-managed explicitly, so no implicit column behavior is wanted.
- `date` -> `DATE`; `json` -> `JSON`.
- `uuid` / `Id` -> **`UUID`** (native MariaDB type, D1). The implicit `id` is app-generated, so it
  gets no SQL `DEFAULT`; a relation's FK column takes the target's PK type (default `UUID`).
- A to-many scalar (`text[]`) has no columnar form -> `JSON` (a JSON array).

Relations emit the FK *column* (`<field>_id`) but **no** `FOREIGN KEY` constraint — constraints are
opt-in (relations.md). `(unique)` -> a `UNIQUE` constraint; `@index` -> an inline `KEY` / `UNIQUE
KEY` whose column list resolves relation fields to their FK columns. All identifiers are backtick-
quoted (legacy/reserved names like `order`, `user` are common, D8).

## D11 — SQL DML mapping (MariaDB, query SELECTs)
How a `query` lowers to SQL (`based gen sql` query section, `based-codegen::sql::dml`). The read
side; mutations are a later increment.
- **Root alias = the table name** (`FROM \`order\``); every join is aliased `j_<path_prefix>` with
  dots -> `_` (`address.city` -> `j_address_city`), so joins dedupe by the path prefix they traverse
  and a shape + a `where` reaching the same relation share one join.
- **Relation reaches become joins; a terminal relation is its FK column.** `placed_by.name` joins
  `user` and reads `name`; a single-segment relation in a filter (`org = $org`) compares the local
  FK (`order.org_id`), no join. Forward-optional -> `LEFT JOIN`, forward-required -> `JOIN`
  (inner), inverse/to-many -> `LEFT JOIN`.
- **Soft-delete + `@scope` injection is the headline guarantee** (soft-delete.md, auth.md). The
  tombstone predicate is added to the root `WHERE` and to *every joined table's `ON`* (keeping a
  `LEFT JOIN` a left join). Covered subset -> predicate: timestamp/date `IS NULL`, bool `= FALSE`.
  `@scope(pred)` is lowered like any `where` and ANDed on. The user writes neither.
- **Parameters render as named `:name` placeholders** (`$ctx.org` -> `:ctx_org`), not MariaDB's
  native positional `?`. Named keeps the emitted template legible (readable > terse, CLAUDE.md);
  the generated client (M4) translates to the driver's binding form.
- **Sort cascade**: query `order (...)` > model `@sort` > none (sema lints the empty `list`). Keyset
  pagination (a `page` without `offset`) appends `\`root\`.\`id\` ASC` as the unique tiebreaker —
  shown in the SQL, never written in source (principle 8, pagination.md).
- **Pagination**: `page (N)` -> `LIMIT N`; `offset` -> `... OFFSET :offset`; `with count` -> a
  second `SELECT COUNT(*)` over the same FROM/JOIN/WHERE (live rows, no LIMIT). The keyset cursor
  comparison itself is a runtime concern and is *not* in the generated base SELECT.
- **Bare bool** in a predicate (`where (... and active)`) -> `\`t\`.\`active\` = TRUE`. Operators:
  `~` -> `LIKE`, `in` -> `IN`, `has` -> `MEMBER OF` (JSON array containment), the rest verbatim.
- *Deferred, rendered visibly rather than wrong*: nested shape sub-objects are skipped (need JSON
  aggregation); a named-filter call in `where` becomes `TRUE /* filter … deferred */` (filter bodies
  aren't resolved against the call site yet, sema resume #2).
