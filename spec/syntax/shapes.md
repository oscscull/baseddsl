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

## Inline legal
`{ name, email, city = address.city.name }`

## Conventional full shape
A model may define `shape full { ... }` for the stereotyped complete view -> `-> full`.

## Rule
No filtering inside a shape. Zero `where`. No sort either (sort is a row property — see sorting.md). Keeps shape/filter/sort orthogonal; one shape serves every query. Selects only the columns named (efficient).
