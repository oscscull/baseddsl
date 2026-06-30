# commerce example

Worked reference in the **recommended** convention (decisions.md D6/D9) — not a
required layout. One file extension (`.bsl`); the compiler only globs `**/*.bsl`,
so you may divide files however you like. Here:

- `model.bsl` — the domain's model, with its shapes (`from X`) alongside it.
- `queries.bsl` — that domain's queries / mutations / filters (present only where there are any).

```
org/         model.bsl
user/        model.bsl
membership/  model.bsl                 # join model (org, user) -> role
product/     model.bsl  queries.bsl    # model.bsl also holds shape ProductCard
order/       model.bsl  queries.bsl    # model.bsl also holds shape OrderCard
order_item/  model.bsl
```

You could equally split shapes into `shapes.bsl`, collapse a domain into one file,
or group by feature — all partitions parse to the same schema.

Exercises: multi-relation-to-same-model (order.placed_by / fulfilled_by),
self-ref inverse (user.invited_users), join-model m2m (membership), soft-delete,
opt-in inverses, index decl, model/relation/query sort tiers, query tiers
(bare / per-param `->` / full body), and a mutation.
