@soft_delete(deleted_at)
OrderItem {
  deleted_at: timestamp?
  order:      Order
  product:    Product
  quantity:   int
  unit_price: int
}
