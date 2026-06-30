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

## Inline legal
`{ name, email, city = address.city.name }`

## Conventional full shape
A model may define `shape full { ... }` for the stereotyped complete view -> `-> full`.

## Rule
No filtering inside a shape. Zero `where`. No sort either (sort is a row property — see sorting.md). Keeps shape/filter/sort orthogonal; one shape serves every query. Selects only the columns named (efficient).
