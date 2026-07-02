# syntax/models.md

Principles: 2 (no hidden fields), 3 (delimiters), 8 (show derived).

## File
One model per file. References out are fine; editing a model never mutates another model's file. Blocks `{ }`. Items separated by newline or comma. Layout free.

## Field line
Uniform for columns and relations: `name: Type (modifiers)`

## Types
- Primitives lowercase: `text int bool timestamp date json uuid`
- Models capitalized: `User Order`
- Casing is load-bearing + committed: capital = relation, lowercase = column. Never lowercase a model or capitalize a primitive.

## Qualifiers
- Type-intrinsic ride the type: `?` = nullable/optional, `[]` = to-many. (`User?`, `text[]`, `Order[]`)
- Behavioral go in parens: `(unique)`, `(default "x")`, `(default now())`
- Split is intentional: cardinality/optionality = type-shape; constraints = field-qualifier.

## Defaults
- `id` implicit. Declare a key only if non-standard (deviation is visible).
- Not-null default. `?` opts into nullable.

## Decorators (model-level)
Stacked `@decorator` lines above the model. Never positional keywords on the model line. Extensible: `@soft_delete(...)`, `@sort(...)`, `@scope(...)`, `@created(field)` / `@updated(field)` (mark a declared timestamp engine-managed — timestamps are never implicit; decisions.md D2), `@table("legacy_name")` (legacy table alias — D3/D8). Tenant scoping is not its own decorator — express it with `@scope` (auth.md).
```
@soft_delete(deleted_at)
@scope(org = $ctx.org)
Order {
  org:    Org
  status: text (default "pending")
  total:  int
}
```
Stack = at-a-glance summary of model nature, read before body. Unknown `@foo` still recognizably a modifier.
