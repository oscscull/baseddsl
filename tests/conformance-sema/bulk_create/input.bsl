# BW1: a `shape` doubles as a bulk-input type. `create Model[] from $rows` (bulk) and
# `create Model from $row` (single) pull column values from a shape-typed param, written
# verbatim (presence-driven); a to-one relation is FK-linked with an inline `{ key }` block.
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)

Category { id: Id, name: text }

@scope Tenant
Product {
  id: Id
  org: Org
  category: Category
  sku: text
  name: text
  price: int
  @index(org)
  @index(category)
}

shape ProductIn from Product {
  sku
  name
  price
  category { id }
}

mutation bulk_add_products(rows: ProductIn[]) -> ok scoped Tenant {
  create Product[] from $rows;
}

mutation add_one_product(row: ProductIn) -> ok scoped Tenant {
  create Product from $row;
}
