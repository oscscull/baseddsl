# Reads + writes on Order. Order is `@scope Tenant` (order.bsl), so every callable that
# touches it names the scope with `scoped Tenant` — the visible half of the contract.

# get, keyed on the unique `id`.
query order_by_id(id) -> OrderCard scoped Tenant;

# The Tenant scope already filters to the caller's org, so a plain `list` *is*
# "my org's orders" — the scope predicate + `@sort(placed_at desc)` do all the work.
query my_orders() -> OrderCard[] scoped Tenant { list Order; }

# Keyset-cursor pagination (L2/D56): walk the whole set two rows at a time. The response
# is `{ rows, cursor }`; feed `cursor` back to fetch the next page (null cursor = done).
query recent_orders() -> OrderCard[] scoped Tenant {
  list Order order (placed_at desc) page (2);
}

# `org` is Tenant-scope-managed on create — the engine sets it from `$ctx`, never a
# param, so an order can't be placed into another tenant's scope.
mutation place_order(buyer: Id, total: decimal(12, 2)) -> OrderCard scoped Tenant {
  create Order { placed_by = $buyer, total = $total };
}

# `delete` on a @soft_delete model is the soft action (tombstone, never real DELETE).
# The response is the tombstoned row read back in its declared shape (D58).
mutation cancel_order(id: Id) -> OrderCard scoped Tenant {
  delete Order where (id = $id);
}

mutation restore_order(id: Id) -> OrderCard scoped Tenant {
  restore Order where (id = $id);
}
