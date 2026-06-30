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

## D8 — Reserved words (globally reserved) + legacy alias
Keywords are **globally reserved**, not contextual. Reserving is simpler and gives clearer
errors than position-sensitive keywords. Set:
`get list create update delete restore hard tx where order page offset count with guard
from shape query mutation filter unindexed unsafe on column table by has in not and or
true false null now`.

A DSL identifier may not BE a reserved word. But the legacy DB we target may have columns or
tables named with one (`order`, `status`, `group`, …) — we must not require it to change. So
the escape is an alias: name the field/model with a legal identifier and map it to the real
name via `(column "order")` / `@table("order")` (D3). The mapping is greppable and lives in
one place (principle 4). This is the "ugly but explicit" path; it never blocks adopting a
legacy schema.
