# Named nested projection: `placed_by -> UserRef` expands the referenced shape in
# place (one nominal type per site). Negatives: an unknown shape (E0132), a shape
# whose model mismatches the relation target (E0133), and a reference cycle (E0134).
Org {
  name: text
}

User {
  name:          text
  email:         text
  org:           Org
  placed_orders: Order[] (Order.placed_by)
}

Order {
  placed_by: User
  total:     int
}

shape UserRef from User { name, email }
shape OrgRef from Org { name }

shape OrderDetail from Order {
  total
  placed_by -> UserRef
}

shape BadUnknown from Order { placed_by -> Missing }
shape BadModel from Order { placed_by -> OrgRef }

shape LoopA from Order { placed_by -> LoopB }
shape LoopB from User { placed_orders -> LoopA }

query order_detail(id) -> OrderDetail;
