# model + its read contract live together; access layer is in queries.bsl
# created_at is declared + @created (timestamps are never implicit, D2);
# product.queries sorts by it.
@soft_delete(deleted_at)
@created(created_at)
Product {
  id:         Id
  deleted_at: timestamp?
  created_at: timestamp
  org:        Org
  name:       text
  sku:        text (unique)
  price:      int
  active:     bool (default true)
  @index(org, active)
}

shape ProductCard from Product {
  name
  sku
  price
}
