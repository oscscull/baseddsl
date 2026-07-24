# syntax/shapes.md

Principles: 2, 3 (braces not indentation), 4.

Shape = projection over a traversal. `shape Name from Model { ... }`

## Field forms
- Bare field = local, same-name (silent: single meaning)
- `out = path` = reach-and-rename in one operator. `=` covers projection + traversal + aliasing. The `=` lines are exactly the cross-relation reaches.
```
shape UserCard from User {
  name
  email
  city = address.city.name
}
```

## Nesting = brace block (never indentation)
Expands a relation into a sub-object; paths inside are relative to that model.
```
shape UserDetail from User {
  name
  address { street, city = city.name }
}
```

## Nesting by named-shape reference
A nest may reference a top-level shape by name instead of spelling its fields inline
(`->` = "connects to", as in relations and param-binding):
```
shape UserRef from User { name, email }

shape OrderDetail from Order {
  status
  placed_by -> UserRef
}
```
The referenced shape's `from` model must equal the relation's target (compile error
otherwise). The reference is a pure column-list expansion — same rows, same SQL as the
inline nest; child soft-delete/`@scope` stay governed by the nest context — but the
generated client/OpenAPI types the field as the *named* shape (`UserRef`), so every
query and `db→props` mapper shares one nominal type instead of per-parent anonymous
ones. Works on to-one and to-many relations, recurses (a referenced shape may itself
nest, inline or by name — a reference cycle is a compile error). `full` is per-model
and is never referenced this way.

**Reference for a shared type; inline when you mean to trim.** If one shape is forced
to serve two consumers with different needs, split it into two shapes.

## Far-side flattening projection (many-to-many)
A derived field (`=`) may name a relation **path** through a to-many junction edge and out
a forward edge to the far side, with a projection body — returning the **distinct** far-side
rows directly, the junction hidden:
```
shape StudentCourses from Student {
  name
  courses = enrollments.course { title }   # -> courses: [ { title }, … ]
}
```
`enrollments` is a to-many inverse edge (into the junction), `course` a forward edge to the
far model; `courses` is `Vec<Course>`, each far row once (distinct on its primary key — a
duplicate junction link never multiplies it). Order is unspecified unless the far model
declares `@sort`. The far model's and the junction's `@scope` / `@soft_delete` both ride the
subquery, so tombstoned/out-of-scope rows are excluded. The body composes (further nests /
flattens). A keyless (`@no_id`) far model is a compile error. See relations.md (Many-to-many)
for the full contract; implicit-junction sugar stays rejected.

## Aggregate projections
A `=` value may be an **aggregate** over the shape's rows instead of a reach:
`count()`, `sum(col)`, `avg(col)`, `min(col)`, `max(col)`. A shape with any
aggregate field is an *aggregate shape* — a projection over **groups** of rows, not
rows (queries.md pairs it with `group by` / `having`).
```
shape BuyerStats from Order {
  buyer = placed_by        # a group column (a reach)
  orders = count()         # how many rows in the group
  revenue = sum(total)     # over the numeric family
}
```
- `count()` takes no argument (rows in the group) → `int`, never null.
- `sum` / `avg` take one numeric column (`int` / `float` / `decimal`). `sum` keeps the
  column's numeric type; `avg` is always `float`.
- `min` / `max` take one *comparable* column (numeric, `timestamp`, `date`, `text`) and
  keep its type.
- Every aggregate but `count()` is **nullable** (an all-null or empty group aggregates to
  null) — the projected type is `T?`.

An aggregate shape is **flat**: it neither nests a relation nor is nested/referenced by
another shape (a group is not a row, so it has no sub-objects). A non-aggregate projected
column must be a `group by` column of the query using the shape (queries.md); otherwise the
shape is only usable as a whole-table aggregate (one row, no `group by`). An aggregate shape
is never a mutation return (a write reads back a written row, not a group).

## Inline legal
`{ name, email, city = address.city.name }`

## Conventional full shape
A model may define `shape full { ... }` for the stereotyped complete view -> `-> full`.

## Rule
No filtering inside a shape. Zero `where`. No sort either (sort is a row property — see sorting.md): a to-many nest's array order comes from the sort cascade for that traversal (relation `@sort` > target model `@sort`). Keeps shape/filter/sort orthogonal; one shape serves every query. Selects only the columns named (efficient).
