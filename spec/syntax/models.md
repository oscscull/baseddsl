# syntax/models.md

Principles: 2 (no hidden fields), 3 (delimiters), 8 (show *cost-free* derived facts only).

## File
One model per file. References out are fine; editing a model never mutates another model's file. Blocks `{ }`. Items separated by newline or comma. Layout free.

## Field line
Uniform for columns and relations: `name: Type (modifiers)`

## Types
- Primitives lowercase: `text int bool timestamp date time json uuid float decimal bytes`
- Primary-key strategy types (valid as `id`; `serial` also as a composite `@key` part, see Defaults): `uuid ulid serial`
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

### Time + binary
| type | form | DDL (MariaDB / SQLite / Postgres) | wire | client |
|------|------|-----------------------------------|------|--------|
| `time` | time of day, no date (`HH:MM:SS[.ffffff]`) | `TIME` / `TEXT` / `TIME` | JSON **string** | `Time` (= `String`) |
| `bytes` | binary blob | `BLOB` / `BLOB` / `BYTEA` | JSON **base64 string** | `Bytes` (= `String`) |

- **`time`** is a bare time of day, distinct from `timestamp` (an instant) and `date`. It rides the
  wire as its `HH:MM:SS` string (a fractional part is kept), and — like `timestamp`/`date` — it is
  **ordered**: `= != < > <= >= in` and `min`/`max` all apply. SQLite has no native `TIME`, so it
  degrades to `TEXT` (the zero-padded string compares lexicographically, which is chronologically
  correct); the production dialects use a real `TIME`. A `time` column may take a **string-literal
  default** (`(default "00:00:00")`); a non-string default is `E0313`.
- **`bytes`** is a binary blob. It rides the wire as a **base64** string (never a raw JSON byte
  array), lossless; the client carries it as its base64 `String`, and the engine base64-decodes it
  to raw bytes only at the driver bind (and re-encodes a read value). It is **equality-only** —
  `= != in`; an ordered comparison (`< > <= >=`) or a `sum`/`avg`/`min`/`max` on it is `E0150`/`E0241`
  (a blob has no meaningful order). A `bytes` column **cannot carry a literal default** (there is no
  source spelling for a blob) — that is `E0314`; set it from a raw migration or a DB default, or make
  it nullable (`?`). A `bytes` field inside a **to-many array** projection is unsupported on SQLite
  (its JSON functions cannot carry a `BLOB`) — project it as a flat/top-level field there.

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

### Generated columns — `name = <expr>`
A **generated column** is a stored, derived row property. It is written as a model field with
`=` instead of `:`, reusing the shape computed-expression language — arithmetic (`+ - * /`),
string concat (`||`), and `case when … then … else … end`:

```
Product {
  price:    decimal
  discount: decimal
  net = price - discount
}
# → net DECIMAL GENERATED ALWAYS AS (price - discount) STORED
```

It lowers to a SQL **native generated column** (`GENERATED ALWAYS AS (<expr>) STORED` — Postgres
12+, MySQL 5.7+/MariaDB, SQLite 3.31+). Because it becomes a **real column**, everything downstream
just works with no special-casing: **project** it (a bare shape field), **filter** it
(`where (net > 10)`), **sort** it (`order (net desc)`), **`@index`** it, keyset-paginate on it.

- **STORED** by default (a real, indexable column). VIRTUAL is out of scope for v1.
- **Type + nullability are inferred** from the expression: arithmetic promotes over the numeric
  family (decimal > float > int), `||` is text, a `case` unifies its branches; a column operand
  carries its own type and nullability.
- The expression sees only the **row's own columns** — no relation reaches (`E0340`), no other
  generated column (`E0346`), no aggregates, no parameters (`E0342`). A stored generated column
  can only depend on the row it belongs to.
- Operand typing: an arithmetic operand must be numeric (`E0343`), a `||` operand text (`E0344`),
  a `case`'s branches must unify (`E0345`); an unknown column is `E0341`.
- Its **value is derived, never written**: assigning it in a `create`/`update` is `E0347`, and a
  `create` never requires it (even though it is NOT NULL and carries no default).
- Migrations: a generated column adds, drops, and — because no dialect can alter a generation
  expression in place — **re-derives (drop + re-add)** when its expression changes.

Pick the form by whether you need to filter/sort/index by the value: a **shape computed field**
(shapes.md) is a query-local, projection-only derived *output* (never stored); a **model generated
column** is stored derived *data*.

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
until the INSERT runs, but a bound create re-selects its written row, so a `tx` step **can**
reference it via `$name.id` — the read-back threads the DB-generated id into the later step
(transactions.md / mutations.md).

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

**A `serial` part (DB-generated).** One part of a **composite** key may be `serial` — a
DB-generated sequence, the `(device, seq)` time-series / append pattern. That part is
*engine-generated*: the `create` omits it (it is not a required input), and the engine reads
the DB-assigned value back to complete the key tuple (Postgres `RETURNING`, MariaDB
`LAST_INSERT_ID()`). The structured id carries it like any other part (`ReadingId { device,
seq }`). A table has one auto-increment column, so two `serial` parts is `E0282`; a
single-column `@key(seq)` with `seq: serial` stays `E0267` (write `id: serial`). On MariaDB a
non-leading serial part gets a covering index automatically. SQLite has no auto-increment for
a non-sole-PK column, so a `serial` composite part needs the `raw` hatch there.
```
@key(device, seq)
Reading { device: Device  seq: serial  value: int }
# → PRIMARY KEY (device_id, seq); `seq` is DB-assigned; create sets only device + value
```

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
