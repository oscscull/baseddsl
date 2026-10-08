# Add SQL while naming the boundary you own

Task: express one computed value or a whole query beyond the DSL. With the
tutorial's Item model, a shape leaf keeps the engine-built outer query:

```bsl
shape ItemLength from Item {
  id
  length = raw`length(name)`
}
query item_lengths() -> ItemLength[] scoped Author { list Item order (id); }
```

Root and modeled joins keep injected scope/soft-delete predicates. The raw
expression is opaque: any subquery you place inside it owns its own table filters.
For example, `(SELECT COUNT(*) FROM item)` can count all owners even if outer
rows are confined. Prefer modeled relations, or explicitly filter internal tables.

A whole raw body must use a flat shape and opt out for a scoped model:

```bsl
shape ItemFlat from Item { id, name }
query named_items(owner: uuid, name: text) -> ItemFlat[] unscoped("host-authorized raw lookup") {
  raw`SELECT id, name FROM {table} WHERE owner = ${owner} AND name = ${name}`;
}
```

`${owner}` and `${name}` bind parameters; they are not SQL string concatenation.
The host must derive and authorize `owner`. You own all scope and soft-delete
filters, ordering, and dialect portability in this query. Declared result types
do not verify the SQL's authorization or its returned columns. A raw body cannot
promise `scoped`, infer untyped parameters, return nested shapes, or stream.

[Raw semantics](../../spec/syntax/raw.md) are authoritative. The
[scope matrix](../scope-contracts.md) proves outer-leaf enforcement and deliberate
cross-owner visibility inside raw subqueries and whole bodies. The helpdesk's
[overdue predicate](../../examples/axum-helpdesk/schema/ticket/queries.bsl) is a
small server-specific leaf; it is not portable SQLite interval syntax.
