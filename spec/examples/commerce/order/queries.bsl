# bare / per-param `->` / full-body query tiers, plus a mutation. Order is `@scope`d
# (model.bsl), so every query here is org-scoped from `$ctx` automatically.
query order_by_id(id) -> OrderCard;
query orders_by_buyer(user -> placed_by) -> OrderCard[];

# `@scope` already filters to the caller's org, so a plain `list` *is* "my org's
# orders" — the scope predicate + the `@sort(placed_at desc)` do all the work.
query my_org_orders() -> OrderCard[] { list Order; }

# Admin/support: read across *every* org. That is cross-scope, so it must opt out of
# the standing scope explicitly — a greppable, linted escape hatch (auth.md / D32). The
# `org` param is a Handle-1 filter value the caller supplies.
query orders_in_org(org) -> OrderCard[] unscoped("admin: cross-org order lookup");

# `org` is `@scope`-managed on create — the engine sets it from `$ctx`, never a param,
# so an order can't be placed into another tenant's scope (D32).
mutation place_order(buyer: Id, total: int) -> OrderCard {
  create Order { placed_by = $buyer, total = $total };
}
