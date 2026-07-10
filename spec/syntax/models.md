# syntax/models.md

Principles: 2 (no hidden fields), 3 (delimiters), 8 (show derived).

## File
One model per file. References out are fine; editing a model never mutates another model's file. Blocks `{ }`. Items separated by newline or comma. Layout free.

## Field line
Uniform for columns and relations: `name: Type (modifiers)`

## Types
- Primitives lowercase: `text int bool timestamp date json uuid float decimal`
- Models capitalized: `User Order`
- Casing is load-bearing + committed: capital = relation, lowercase = column. Never lowercase a model or capitalize a primitive.

### Numbers
| type | range / form | DDL (MariaDB / SQLite / Postgres) | wire | client |
|------|--------------|-----------------------------------|------|--------|
| `int` | 64-bit signed integer | `BIGINT` / `INTEGER` / `BIGINT` | JSON number | `i64` |
| `float` | 64-bit binary floating point | `DOUBLE` / `REAL` / `DOUBLE PRECISION` | JSON number | `f64` |
| `decimal(p, s)` | fixed base-10, precision `p`, scale `s` | `DECIMAL(p,s)` / `TEXT` / `NUMERIC(p,s)` | JSON **string** | `rust_decimal::Decimal` |

- **`decimal` is for exact values (money).** `decimal(p, s)` — `p` total digits, `s` after the
  point (`total: decimal(12, 2)`); `1 ≤ s ≤ p ≤ 38`. Bare `decimal` defaults to `decimal(38, 9)`.
  It rides the wire as a **JSON string** (`"9.99"`) and never rounds through a float, so no digit
  is lost; a `default` is preserved byte-exact (`default 9.99` stays `9.99`). SQLite stores it as
  `TEXT` (exact string; comparison is lexicographic there — production dialects use a true numeric
  `DECIMAL`/`NUMERIC`). The generated client needs the `rust_decimal` crate (feature `serde-str`).
- **`float`** is one type (double precision). `double` is not a separate spelling today; it can be
  added later as an alias. Use `decimal` when exactness matters — `float` is inexact.
- `int`, `float`, and `decimal` share one **numeric** family: a numeric literal compares/assigns to
  any of them, and they inter-compare with `= != < > <= >= in`.

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
