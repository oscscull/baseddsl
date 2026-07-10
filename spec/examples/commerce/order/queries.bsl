# bare / per-param `->` / full-body query tiers, plus a mutation. Order is `@scope Tenant`
# (model.bsl), so every callable that touches it names the scope with `scoped Tenant` —
# the visible half of the both-sides contract (auth.md Handle 2 / D46).
query order_by_id(id) -> OrderCard scoped Tenant;
query orders_by_buyer(user -> placed_by) -> OrderCard[] scoped Tenant;

# Returns `OrderDetail` (model.bsl), whose buyer nests by named-shape reference — the
# generated client types `placed_by` as the shared `UserRef`, not a per-query struct.
query order_detail(id) -> OrderDetail scoped Tenant;

# The Tenant scope already filters to the caller's org, so a plain `list` *is* "my org's
# orders" — the scope predicate + the `@sort(placed_at desc)` do all the work.
query my_org_orders() -> OrderCard[] scoped Tenant { list Order; }

# Admin/support: read across *every* org. That is cross-scope, so it must opt out of
# the standing scope explicitly — a greppable, linted escape hatch (auth.md / D32). The
# `org` param is a Handle-1 filter value the caller supplies.
query orders_in_org(org) -> OrderCard[] unscoped("admin: cross-org order lookup");

# `org` is `Tenant`-scope-managed on create — the engine sets it from `$ctx`, never a
# param, so an order can't be placed into another tenant's scope (auth.md / D46).
mutation place_order(buyer: Id, total: decimal(12, 2)) -> OrderCard scoped Tenant {
  create Order { placed_by = $buyer, total = $total };
}
