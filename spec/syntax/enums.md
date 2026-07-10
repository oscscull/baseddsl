# syntax/enums.md

Principles: 2 (nothing consequential by omission), 3 (delimiters), 5 (push a semantic in only if the compiler can guarantee something).

## What an enum is

A closed set of named values, and a first-class **scalar type**. An enum-typed field is a stored column (not a relation): its value is one of the enum's variants, checked at compile time and — as defense in depth — by the database.

```
enum Status   { pending, paid = "PAID", shipped, cancelled }
enum Priority { low = 0, medium = 1, high = 2 }

Order {
  status:   Status   (default pending)
  priority: Priority (default low)
  total:    int
}
```

- **Name** — UpperCamel, like a model or shape. It shares the **type-name namespace** with models, shapes, and scopes; a collision is a duplicate (`E0106`).
- **Variants** — lowercase snake identifiers, comma- or newline-separated (like every other block, the separators are insignificant). A variant is a bare identifier, optionally with an explicit wire value: `IDENT [ "=" ( STRING | INT ) ]`. The **name** is always the identifier — it yields the client's Rust variant, go-to-definition, and rename; the value (when written) is what the wire and the database store.

An enum declares no behaviour, only a value set. Whatever *means* a status transition is app logic — it lives in the host language (principle 5); the enum only lets the engine guarantee the column holds a member.

## Kind: inferred from the variant values

An enum is a **string enum** or an **int enum**, inferred from its variants:

- **String enum** — no variant has an int value. Each variant is bare (`pending` → wire `"pending"`) or has an explicit string (`paid = "PAID"` → wire `"PAID"`, name ≠ value). Mixing bare and explicit-string variants is fine.
- **Int enum** — every variant has an explicit int value (`low = 0, medium = 1, high = 2`). No bare or string variant is permitted in an int enum.

Mixing an int variant with a bare/string one in the same enum is a `E0156` (the kind would be ambiguous). Two variants sharing a wire value (two strings, or two ints) is `E0157` (the stored value would be ambiguous). Two variants sharing a *name* is `E0104` (a repeated member).

## Field usage

`status: Status` is an UpperCamel type reference. Sema disambiguates by what the name resolves to:

- resolves to an **enum** ⇒ a **scalar column** (stored value constrained to the variants);
- resolves to a **model** ⇒ a **relation / FK** (unchanged).

Optionality and defaults ride the field as usual:

- `status: Status?` — a nullable enum column.
- `status: Status (default pending)` — the default is a **bare variant identifier**, checked to be a member of the enum (`E0155`; a bare-identifier default on a non-enum column is the same `E0155`). Codegen renders the DB column default as the variant's wire value.

## Values in predicates and writes

A variant appears as a **bare identifier** in value position — always by its **name**, never by its raw value — resolved by sema against the enum type of the compared / assigned column:

```
query paid_orders() -> OrderCard[] { list Order where (status = paid); }
query urgent()      -> TicketRow[] { list Ticket where (priority >= medium); }
mutation mark_paid(id: Id) -> OrderCard { update Order where (id = $id) { status = paid } }
```

A variant that is not a member of the column's enum — including a variant borrowed from a *different* enum — is `E0154`. In the AST a variant value is an ordinary single-segment `Path`; sema resolves it as a variant only when the compared / assigned column is enum-typed (otherwise it stays a column path). This keeps the value grammar unchanged — no new `Value` node — and localizes enum awareness to the two sites that know the column's type. Codegen emits the variant's **wire value** (a quoted string, or a bare integer), so the runtime needs no enum awareness.

**Operators by kind.** A string enum allows `= != in` (equality only — its values have no order). An int enum additionally allows the ordered comparisons `< > <= >=` (it is numeric). An ordered comparison on a *string* enum column is `E0158`.

## Wire representation

- String enum → the string (`"paid"`, or the explicit `"PAID"`).
- Int enum → the JSON number (`2`).

The runtime carries the shaped value accordingly (a string, or a number). A value the database returns that is not a variant surfaces as a typed decode error on the client, never a panic.

## Database representation (per dialect)

A single **column + named CHECK** per enum column, through the same `Dialect` type-map seam every scalar uses.

| kind    | dialect  | column type    | constraint |
|---------|----------|----------------|------------|
| string  | MariaDB  | `VARCHAR(255)` | `CONSTRAINT ck_<table>_<col> CHECK (col IN ('v1', …))` |
| string  | SQLite   | `TEXT`         | `CONSTRAINT ck_<table>_<col> CHECK (col IN ('v1', …))` |
| string  | Postgres | `TEXT`         | `CONSTRAINT ck_<table>_<col> CHECK (col IN ('v1', …))` |
| int     | MariaDB  | `BIGINT`       | `CONSTRAINT ck_<table>_<col> CHECK (col IN (0, 1, …))` |
| int     | SQLite   | `INTEGER`      | `CONSTRAINT ck_<table>_<col> CHECK (col IN (0, 1, …))` |
| int     | Postgres | `BIGINT`       | `CONSTRAINT ck_<table>_<col> CHECK (col IN (0, 1, …))` |

The CHECK lists the **wire values** (a renamed string variant checks `'PAID'`, not `'paid'`). Why a plain column + CHECK rather than a DB-native enum type (MariaDB inline `ENUM(…)`, Postgres `CREATE TYPE … AS ENUM`): migration simplicity. A native enum makes a variant add an `ALTER TYPE … ADD VALUE` / `MODIFY COLUMN` — non-transactional on older Postgres, unable to *remove* a value, and a second type map that can drift from `based gen sql`. SQLite has no native enum at all. One uniform representation keeps the three dialects honest through the same type-map seam and makes a variant change a diffable column change.

## Migrations

The neutral snapshot records an enum column's type as `enum(v1,v2,…)` (string enum, its wire values) or `enum:int(0,1,…)` (int enum), so **adding or removing a variant OR changing an enum's kind (string ↔ int) is a diffable change** (the type string differs → an `alter column`). A from-scratch migration (`0001_init`) emits the same column + CHECK the fresh DDL does. Rendering an in-place variant change back to the CHECK constraint per dialect (drop + add) is a later refinement; the minimum guarantee is that the change is captured and never crashes the diff.

## Client + OpenAPI

- `based gen client` emits a real Rust `enum`:
  - **String enum** — serde-renamed to the wire strings (`#[serde(rename = "PAID")] Paid`), so `Status::Paid` (de)serializes as `"PAID"`.
  - **Int enum** — explicit discriminants (`enum Priority { Low = 0, … }`) plus a hand-rolled `Serialize`/`Deserialize` that (de)serializes as the integer. **No new dependency** (no `serde_repr`): the manual impl `serialize_i64`s the discriminant and matches the incoming `i64` back to a variant, an unknown value becoming a serde decode error.
  - An enum-typed field/output takes this type instead of `String`.
- `based gen openapi` emits `{ type: string, enum: [strings] }` for a string enum, `{ type: integer, enum: [ints] }` for an int enum.

A value the database (somehow) returns that is not a variant surfaces as a client **decode error**, never a panic — the same typed-error discipline every other decode follows.

## Diagnostics

| code | meaning |
|------|---------|
| `E0104` | two variants of an enum share a name (a repeated member) |
| `E0106` | enum name collides with a model / shape / scope / enum |
| `E0154` | a `where` / `create` / `update` value is not a variant of the column's enum |
| `E0155` | a field's `default <variant>` is not a member of its enum (or a bare-identifier default sits on a non-enum column) |
| `E0156` | an enum mixes an int-valued variant with a bare / string one (kind is ambiguous) |
| `E0157` | two variants of an enum share a wire value (a string or an int) |
| `E0158` | an ordered comparison (`< > <= >=`) on a string enum column |
