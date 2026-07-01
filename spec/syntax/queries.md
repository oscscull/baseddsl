# syntax/queries.md

Principles: 1, 2, 3 (delimited clauses, not space-runs).

## Queries are named typed callable units
A query is a signature: name, typed inputs, typed output, optional body. As complete a contract as a model. The signature is what the client surface is generated from (see calling.md).

## Bare form (arrow-function style)
Params ARE the filter. No body, no verb, no `where` needed. Param maps to its same-named column by equality; params are equality-AND composed.
```
query order_by_id(id) -> OrderCard;          // get Order; where id = $id
query orders(user, company) -> OrderCard[];  // list; where user = $user and company = $company
query line_items(order) -> OrderItem[];      // list; where order = $order
```

Four inferences, each single-meaning + safe (deviation forces the body, so principle 2 holds):
- verb: singular return -> `get`, `[]` -> `list`
- param type: from the column it maps to (so `(id)` needs no type)
- filter: param = equality on same-named column
- target model: from the return shape (`shape X from Model`)

## Per-param binding (when default doesn't fit)
A param carries its own binding, locally, inside the brackets. Mix freely; equality-AND across params.
```
(id)                      # same-name equality, type inferred
(user: User)              # typed, same-name equality
(user -> author)          # binds via a named relation/edge (disambiguates multi-relations by edge name)
(since: timestamp > created_at)   # explicit column + operator

query posts(user -> author, company -> owner) -> PostShape[];
query posts(user -> author, since: timestamp > created_at) -> PostShape[];
```
`->` = "binds via" (consistent with relation-arrow: "connects to"). Ceiling: one binding per param, equality-AND across params. Anything needing `or`, a cross-param condition, ordering, pagination, or a non-default projection -> drop to the body.

## Full body form
```
query products(org: Id, active: bool = true) -> OrderCard[] {
  list Product
    where (org = $org and active = $active)
    order (created_at desc)
    page (20);
}
```

## Body skeleton (delimited, not space-runs)
Each clause's argument is bounded by `()`, same as field modifiers. Statement ends `;`.
- verb: `get` (one) / `list` (many)
- `where (predicate)`
- `order (field desc|asc)`
- `page (N)`

## $param vs column
`$name` = a signature input. Bare name = a column. `where (id = $id)` = column id equals input id. `$` means "bound parameter" everywhere (also raw.md).

## Cardinality in two layers (both kept)
- Signature `-> OrderCard` / `-> OrderCard[]` = client-facing contract (drives codegen return type).
- Body verb `get`/`list` = engine instruction.
Contract/implementation split, not redundancy.

## Filters
- Operators (small, closed): `= != > < >= <=`, `~` (like), `in`, `has` (array/json containment).
- Compose `and`/`or`/`not` + parens. `and` binds tighter than `or`. No other precedence.
- `get` must be keyed on a unique field (else lint-error).
- Filter paths and projection paths share the same dotted traversal. Forward-edge traversal needs no inverse declaration; only backward traversal does (relations.md).

## Named filters (reuse)
```
filter active = not banned and deleted_at = null;
filter in_city(c) = address.city.name = $c;
```
Same predicate language as `where`, soft-delete injection, and auth scope. One expression type everywhere — never separate grammars. A filter param is referenced as `$c` inside the body — the same `$`-means-bound-parameter rule as everywhere else ("$param vs column", above). A filter has no model of its own; its column paths (`address.city.name`) resolve against whichever model calls it (D14).
