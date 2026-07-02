# A query whose `where` traverses an inverse edge (Order.items -> OrderItem):
# the join key is auto-indexed on the target (OrderItem.order, the FK field —
# DDL lowers it to `inf_..._order_id`). Ordering avoids the nondeterministic list lint.
Order {
  placed_at: timestamp
  items:     OrderItem[]
}

OrderItem {
  order: Order
  qty:   int
}

shape OrderCard from Order { placed_at }

query busy_orders() -> OrderCard[] {
  list Order where (items.qty > 0) order (placed_at desc);
}
