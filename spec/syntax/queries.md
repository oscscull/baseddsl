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
`->` = "binds via" (consistent with relation-arrow: "connects to"). In the `op column` form the **column is the left operand**: `since: timestamp > created_at` binds `created_at > $since` — rows created *after* `$since`. Ceiling: one binding per param, equality-AND across params. Anything needing `or`, a cross-param condition, ordering, pagination, or a non-default projection -> drop to the body.

## Full body form
```
query products(org: Id, active: bool = true) -> OrderCard[] {
  list Product
    where (org = $org and active = $active)
    order (created_at desc)
    page (20);
}
```

A block body may instead be one `raw` backtick block — the whole-query raw level
(raw.md): the SQL is the statement, `${param}` stays bound, the declared shape types
the result columns.

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

A third signature form, `-> stream OrderCard`, is the incremental many-contract — rows
delivered one at a time (exports, large scans). Body verb is still `list`; wire + client
contract in streaming.md.

## Scope acknowledgement (`scoped` / `unscoped`)
If a query's target model is in a scope (`@scope Name`, auth.md Handle 2 / D46), the signature **must**
say so — `scoped Name` to accept the standing filter, or `unscoped("reason")` to opt out. Writing
neither is `E0182` (the contract is too important to be true by omission). Both sit after the return
type; a query reaching a second scoped model names both (`scoped Tenant, Region`). A model with several
`@scope` alternatives (OR, D47) is satisfied by naming **one** of them — a `Post` scoped by page *or*
author: `posts_on_page … scoped Page` / `my_posts … scoped Author`, each fully confined. See auth.md.
```
query order_by_id(id) -> OrderCard scoped Tenant;
query orders_in_org(org) -> OrderCard[] unscoped("admin: cross-org order lookup");
```

## Filters
- Operators (small, closed): `= != > < >= <=`, `~` (like), `in`, `has` (array/json containment). `~` passes its pattern verbatim to SQL `LIKE` — the caller supplies any `%` wildcards.
- `in` takes either a single bound value (`status in $status` — one `$param`) or a parenthesized value list: `status in (open, waiting, $other)`. List elements are ordinary values — literals, enum variants, `$param` references, columns — each checked against the left column's type (an enum column checks variant membership per element, `E0154`; a family mismatch like `total in (1, "x")` is the same error an `=` comparison gives). Lowers to SQL `IN (v, v, …)` with `$param` elements bound as parameters.
- Compose `and`/`or`/`not` + parens. `and` binds tighter than `or`. No other precedence.
- `get` must be keyed on a unique field (else lint-error).
- Filter paths and projection paths share the same dotted traversal. Forward-edge traversal needs no inverse declaration; only backward traversal does (relations.md).

## Aggregations, group by, having
An **aggregate query** returns an aggregate shape (shapes.md — a shape with
`count()`/`sum`/`avg`/`min`/`max` fields). Two body clauses pair with it:
```
query buyer_stats() -> BuyerStats[] {
  list Order
    group by (placed_by)
    having (revenue > 1000)
    order (revenue desc);
}
```
- `group by (col, …)` — the grouping columns. Every **non-aggregate** projected column of
  the return shape must be a `group by` column (the SQL rule, enforced as `E0242`). With no
  `group by` the shape must be **all aggregates** — one whole-table row (a `get`).
- `having (predicate)` — filters **groups** by their aggregates, the same predicate language
  as `where`. Its left operands are the shape's projected names: an aggregate alias
  (`revenue`) or a `group by` column. `where` filters rows *before* grouping; `having`
  filters groups *after* — keep the two distinct.
- `group by` / `having` are legal **only** on an aggregate query (`E0243`).

`where` (row filter), `@scope` injection, and soft-delete all still apply — they narrow the
rows *before* grouping, so a scoped/soft-deleting model aggregates only its live, in-scope
rows without the scope column needing to be grouped. An aggregate query's `order` may name
only `group by` columns; it takes **no** default model `@sort` (an ungrouped sort key is not
a valid grouped column) and **cannot** be paginated (`page` is `E0244` — grouped keyset
paging is deferred).

## Named filters (reuse)
```
filter active = not banned and deleted_at = null;
filter in_city(c) = address.city.name = $c;
```
Same predicate language as `where`, soft-delete injection, and auth scope. One expression type everywhere — never separate grammars. A filter param is referenced as `$c` inside the body — the same `$`-means-bound-parameter rule as everywhere else ("$param vs column", above). A filter has no model of its own; its column paths (`address.city.name`) resolve against whichever model calls it (D14).
