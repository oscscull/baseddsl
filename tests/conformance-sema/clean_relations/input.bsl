# Soft-delete model, forward + inferred-inverse relations, a shape, and the
# three query tiers — exercises table naming, relation kinds, inverse inference,
# the inferred join-key index, and verb/target inference end to end.
@soft_delete(deleted_at)
@sort(placed_at desc)
Order {
  deleted_at: timestamp?
  placed_by:  User
  total:      int
  placed_at:  timestamp (default now())
  items:      OrderItem[]
}

OrderItem {
  order: Order
  qty:   int
}

User {
  name:  text
  email: text (unique)
  orders: Order[] (Order.placed_by)
}

shape OrderCard from Order {
  total
  buyer = placed_by.name
}

query order_by_id(id) -> OrderCard;
query orders_by_buyer(user -> placed_by) -> OrderCard[];
