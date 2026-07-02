# model + its read contract live together; access layer is in queries.bsl
@soft_delete(deleted_at)
@sort(placed_at desc)
Order {
  deleted_at:   timestamp?
  org:          Org
  placed_by:    User
  fulfilled_by: User?
  status:       text (default "pending")
  total:        int
  placed_at:    timestamp (default now())
  items:        OrderItem[]
  @index(org, status)
  @index placed_at
  @index placed_by
}

shape OrderCard from Order {
  status
  total
  buyer = placed_by.name
  org   = org.name
}
