# `for update` — pessimistic locking read (SELECT … FOR UPDATE), a trailing modifier on a
# `get`/`list` block query. Compile-time-confined to transaction clients via the generated
# `TxBound` trait (not visible in this resolution summary). Legal only where the locked
# base-row set is well-defined and single. Diagnostics: E0315 (with `distinct`), E0316
# (aggregate query), E0317 (to-many nest projection), E0318 (`-> stream` query).
Product {
  id:    Id
  sku:   text (unique)
  name:  text
  price: int
  items: LineItem[]
}

LineItem {
  id:      Id
  product: Product
  qty:     int
}

shape ProductRow from Product {
  sku
  name
  price
}

# Clean: a single-row locking read, block form.
query product_for_update(id) -> ProductRow {
  get Product where (id = $id) for update;
}

# Clean: a locking list.
query products_for_update() -> ProductRow[] {
  list Product order (sku) for update;
}

# Clean: `for update nowait` rides the same boundaries as plain `for update`.
query product_nowait(id) -> ProductRow {
  get Product where (id = $id) for update nowait;
}

# Clean: `for update skip locked` on a list.
query products_skip_locked() -> ProductRow[] {
  list Product order (sku) for update skip locked;
}

# E0315: `for update` with `distinct`.
query distinct_lock() -> ProductRow[] {
  list distinct Product order (sku) for update;
}

shape SkuCount from Product {
  sku
  n = count()
}

# E0316: `for update` on an aggregate query.
query agg_lock() -> SkuCount[] {
  list Product group by (sku) for update;
}

shape ProductWithItems from Product {
  sku
  items { qty }
}

# E0317: `for update` on a query projecting a to-many nest.
query nest_lock(id) -> ProductWithItems {
  get Product where (id = $id) for update;
}

# E0318: `for update` on a `-> stream` query.
query stream_lock() -> stream ProductRow {
  list Product order (sku) for update;
}
