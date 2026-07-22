# A query whose `where` traverses an inverse edge (Order.items -> OrderItem): the join
# runs through OrderItem.order, which no `@index` covers — a hard error (E0260). The fix
# is an explicit `@index order` on OrderItem (or a visible `unindexed(…)` opt-out).
Order {
  id: Id
  placed_at: timestamp
  items:     OrderItem[]
}

OrderItem {
  id: Id
  order: Order
  qty:   int
}

shape OrderCard from Order { placed_at }

query busy_orders() -> OrderCard[] {
  list Order where (items.qty > 0) order (placed_at desc);
}
