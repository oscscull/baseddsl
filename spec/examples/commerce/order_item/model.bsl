@soft_delete(deleted_at)
OrderItem {
  id:         Id
  deleted_at: timestamp?
  order:      Order
  product:    Product
  quantity:   int
  unit_price: int
}
