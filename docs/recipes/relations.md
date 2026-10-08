# Return a related parent in a small projection

Task: read an item's parent without exposing every model field. In the tutorial's
canonical schema, `parent: Item?` defines the optional relation; select it in a shape:

```bsl
shape ItemView from Item {
  id
  name
  parent { id, name }
}
```

The generated DTO contains an optional nested parent. Adding a model field does
not automatically add it to this projection. Modeled relation reads retain scope
and soft-delete filters on reached models; a foreign scoped target can be absent
even when a physical reference exists. A database foreign key alone is not an
authorization rule. Explicit ordering is needed for ordered to-many projections.

Run the [embedded tutorial](../embedded-tutorial.md) or
[standalone tutorial](../standalone-tutorial.md), which creates a parent and child
and reads this exact shape. For inverse/composite/many-to-many relations, use the
[relation reference](../reference.md#relations) and [shape reference](../reference.md#shapes).
