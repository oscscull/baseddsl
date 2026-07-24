# syntax/models.md

Principles: 2 (no hidden fields), 3 (delimiters), 8 (show *cost-free* derived facts only).

## File
One model per file. References out are fine; editing a model never mutates another model's file. Blocks `{ }`. Items separated by newline or comma. Layout free.

## Field line
Uniform for columns and relations: `name: Type (modifiers)`

## Types
- Primitives lowercase: `text int bool timestamp date json uuid float decimal`
- Primary-key strategy types (valid only as `id`, see Defaults): `uuid ulid serial`
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

### Opaque columns — `raw("…")`
The primitive set is closed. A column whose DB type the engine does not model — PostGIS
`geometry`, `tsvector`, `inet`, a vendor JSON variant — is declared with the `raw` escape hatch
in **type position**:

```
location: raw("geometry(Point,4326)")?
tags:     raw({ postgres: "tsvector", mariadb: "text" })?
```

A bare `raw("…")` applies to every compile target; the map form names one literal per target
(a target the map omits is `E0270`). The literal string rides **verbatim** into the DDL and into
the neutral snapshot, so the diff is a plain string compare and migrations, sqlite table rebuilds,
and `@was` renames all keep working on the model.

The **value is opaque end to end**: the client sees a plain string, and the engine never reads
into it.

- Writing it is `E0273` — the engine cannot construct a value for a type it does not model. So an
  opaque column must be **nullable (`?`) or `(default …)`**; a required one makes `create` on the
  model impossible (also `E0273`), pointing at the fix.
- `where` / `order` / `group by` / an aggregate on it is `E0271`. Reach it through the `raw` leaf
  instead — a raw predicate term or a raw shape value (`area = raw`ST_Area(location)``, raw.md).
- It has no array form (`raw("…")[]`), and only a **model field** may carry one (never a param
  annotation or a scope term).

Indexing an opaque column is `@index … using <method>` / `@index raw("…")` — indexing.md.

## Qualifiers
- Type-intrinsic ride the type: `?` = nullable/optional, `[]` = to-many. (`User?`, `text[]`, `Order[]`)
- Behavioral go in parens: `(unique)`, `(default "x")`, `(default now())`
- Split is intentional: cardinality/optionality = type-shape; constraints = field-qualifier.

## Defaults
- `id` is **required, written in source**: `id: Id`. A primary key is load-bearing, so
  its omission is not elidable (principle 2) — a model that declares no `id` is an error
  (`E0261`) with a one-key editor autofix that inserts the line. Declare a non-standard key
  (a different type or column) in the same slot: `id: Id (column "account_pk")`.
- A genuinely keyless legacy table — one with **no primary key at all** — opts out with
  `@no_id("reason")` (below). It then forfeits the id-keyed operations.
- Not-null default. `?` opts into nullable.

### Primary-key generation strategy (`id: uuid | ulid | serial`, D110)
A PK's *generation strategy* — how its value comes to exist — is a per-model choice,
written in the `id` type (its generation is consequential, so it is visible, not implied).

- `id: Id` resolves to the project default (`[schema] id` in `based.toml`, default `uuid`).
- `id: uuid` — an app-minted random v4 string (`CHAR(36)` / `TEXT` / native `UUID`).
- `id: ulid` — an app-minted lexicographically-sortable string (`CHAR(26)` / `TEXT`).
- `id: serial` — a **DB-generated** sequential integer: MariaDB/MySQL `BIGINT
  AUTO_INCREMENT`, Postgres `BIGINT GENERATED ALWAYS AS IDENTITY`, SQLite `INTEGER PRIMARY
  KEY AUTOINCREMENT`. The `create` omits the id column; the engine reads the assigned value
  back (`RETURNING id`, or MariaDB's `LAST_INSERT_ID()`) to key the declared-shape return.

A bare `int` (or other numeric) as the `id` is `E0266` — a DB-generated integer key must be
spelled `serial` so its generation is visible; an app-owned key stays a string. `serial` and
`ulid` are strategies, not column types: on any non-`id` column they are `E0267`. **Wire
honesty:** a `serial` id is a JSON *number* (OpenAPI `{type: integer}`), a uuid/ulid id a
string; a relation's FK column mirrors the target PK type (a serial parent → a `BIGINT` FK).
Heterogeneous strategies across one schema are allowed. A serial-created row's id is unknown
until the INSERT runs, so a `tx` step cannot reference it via `$name.id` (`E0268` — make the
referenced model app-minted to reach it across steps).

### `@no_id("reason")` — keyless legacy tables
Some adopted tables have no primary key whatsoever. `@no_id("reason")` records that fact:
the model carries no synthesized `id`, and E0261 is suppressed. The **reason string is
mandatory** (like `unscoped("reason")`) so the PR shows why the key is forfeited — an
empty/missing reason is `E0262`. A keyless model loses the id-keyed operations and the
compiler enforces the loss:
- **No get-by-id.** A `get` keys on some `(unique)` column instead (else the ordinary
  `E0144` fires).
- **Keyset pagination** has no `id` tiebreaker, so a non-`offset` `page` must sort on a
  `(unique)` column for a deterministic cursor — else `E0263` (use `page (…) offset`, or
  add the unique sort key).
- **Create read-back** has no generated id to re-select by, so a declared-shape `create`
  must set a `(unique)` column (the row reads back keyed on it) — else `E0264` (or return
  `-> ok` for no read-back).
- A forward relation **to** a keyless model is `E0265` — there is no `id` for its foreign
  key to reference.

### `@key(f1, f2, …)` — a nominated primary key (natural / composite)
A table whose key is **meaningful existing column(s)** — a `sku`, an `iso_code`, a junction's
`(order, product)` — has a key; it is not `@no_id` (which means genuinely keyless).
`@key(…)` nominates the declared field(s) that form the primary key, in list order, so no
surrogate `id` is synthesized. One field is a **natural single-column key**; two or more form
a **composite key** over those columns.
```
@key(iso_code)
Country { iso_code: text  name: text }
# → PRIMARY KEY (iso_code); no `id`; a relation to Country references iso_code

@key(course, student)
Enrollment { course: Course  student: Student  grade: int }
# → PRIMARY KEY (course_id, student_id); no `id`; the junction has a real key
```
The nominated key is **app-supplied**, not engine-generated (unlike `serial`): the `create`
sets the key column(s) like any other, and the row reads back keyed on the whole key. Each
key field must exist (`E0275`) and be a required, single-valued scalar **or to-one relation**
(its FK column carries the key — `E0276`); an empty `@key()` is `E0278`, a repeated field
`E0279`, and `@key` + `@no_id` is contradictory (`E0277`).

**Single-column key** → the column is the entity's typed id everywhere (`Id<entity::M>` in
the client; an inbound FK mirrors the key's own type, e.g. `TEXT`/`BIGINT`, not a uuid).

**Composite key** → the entity id is a **structured object**, one part per key field. In the
generated client it is a per-part struct (`EnrollmentId { course, student }`), on the wire a
JSON object, in OpenAPI a typed-property object; each part keeps its own typing. A relation
*into* a composite-key model is a **multi-column FK** that auto-expands to `<field>_<part>`
columns (`enrollment: Enrollment` → `enrollment_course_id`, `enrollment_student_id`)
referencing every key column; projected bare, that FK reads back as the structured object.
The composite PK is a unique, covering index — a `get` keyed on the whole tuple is served by
it, and keyset pagination orders by and compares the full key.

### `@schema("name")` — a non-default schema / database namespace
By default a model's table lives in the connection's default namespace. `@schema("name")`
places it in a **named SQL schema** (Postgres) / **database** (MySQL/MariaDB) instead, so a
table that lives outside the default namespace can be consumed and emitted (representability,
principle 9):
```
@schema("analytics")
Event { id: Id  org: Org @fk  note: text }
# → CREATE TABLE "analytics"."event"; every reference is schema.table
```
The name is a **single bare identifier** — a schema (Postgres) or database (MySQL/MariaDB)
name (empty, dotted, or whitespace-carrying → `E0296`; a multi-level `db.schema` qualifier is
not modelled). The qualifier rides **every** table reference: `CREATE TABLE`, all DML
(SELECT/INSERT/UPDATE/DELETE), JOIN targets, index DDL, and an FK `REFERENCES` — including one
*into* a schema-qualified model from another schema. Each part is quoted separately
(`"schema"."table"`), never the dot; column references keep the bare table name as their
correlation. A qualifier is a DB-side placement detail, not part of the entity's typed
identity, so it never appears on the wire / client / OpenAPI. Put a namespace prefix in
`@schema`, never in `@table` — a `.` inside `@table("a.b")` is `E0297`. A model that *moves*
namespace diffs into a reviewable `alter schema` migration step (Postgres `SET SCHEMA`,
MySQL/MariaDB cross-database `RENAME TABLE`; SQLite needs a raw table-rebuild).

## Decorators (model-level)
Stacked `@decorator` lines above the model. Never positional keywords on the model line. Extensible: `@soft_delete(...)`, `@sort(...)`, `@scope(...)`, `@created(field)` / `@updated(field)` (mark a declared timestamp engine-managed — timestamps are never implicit; decisions.md D2), `@table("legacy_name")` (legacy table alias — D3/D8), `@schema("name")` (a non-default schema/database namespace — see above, D113), `@no_id("reason")` (a keyless legacy table — see Defaults), `@no_fk[("reason")]` (opt the whole table out of FK constraints — see relations.md). Tenant scoping is not its own decorator — express it with `@scope` (auth.md).

Field-level, on a forward to-one relation: `@fk[(…)]` opts a relation into a DB `FOREIGN KEY` constraint (with optional `on_delete`/`on_update` actions); `@no_fk` opts one edge out. Presence is resolved against the `[schema] foreign_keys` convention, and a decorator that flips presence against it needs a reason string — full spec in relations.md.
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
