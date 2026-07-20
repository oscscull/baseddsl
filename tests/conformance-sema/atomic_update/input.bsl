# Atomic update expressions: a self-referential arithmetic SET over the model's
# own numeric columns + params lowers to a real SQL expression. Clean on numeric
# columns; E0230 in a `create` (no existing row), E0231 on a non-numeric operand.
Product {
  qty:   int
  price: decimal(10, 2)
  name:  text
}

shape ProductRow from Product { qty, price }

mutation adjust_stock(id: Id, delta: int) -> ProductRow {
  update Product where (id = $id) { qty = qty + $delta };
}

mutation apply_markup(id: Id, factor: decimal(10, 2)) -> ProductRow {
  update Product where (id = $id) { price = (price * $factor) + 1 };
}

mutation bad_create(n: int) -> ProductRow {
  create Product { qty = qty + $n, price = 0, name = "widget" };
}

mutation bad_operand(id: Id, n: int) -> ProductRow {
  update Product where (id = $id) { qty = name + $n };
}
