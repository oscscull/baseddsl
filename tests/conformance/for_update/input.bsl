# `for update` — a trailing pessimistic-locking modifier after the clause list in a block
# query body, on `get` or `list`. Parses to `Statement.for_update`; reprints via `based fmt`.
Product {
  id:    Id
  sku:   text
  name:  text
  price: int
}

shape ProductRow from Product {
  sku
  name
  price
}

# `for update` after a single `where` clause on a `get`.
query product_for_update(id) -> ProductRow {
  get Product where (id = $id) for update;
}

# `for update` after `where` + `order` on a `list` (multi-clause block form).
query cheap_for_update(max) -> ProductRow[] {
  list Product where (price <= $max) order (price) for update;
}

# `for update nowait` — fail fast instead of waiting on a locked row.
query product_for_update_nowait(id) -> ProductRow {
  get Product where (id = $id) for update nowait;
}

# `for update skip locked` — skip already-locked rows.
query available_for_update(max) -> ProductRow[] {
  list Product where (price <= $max) order (price) for update skip locked;
}
