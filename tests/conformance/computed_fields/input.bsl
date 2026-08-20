# Computed shape fields — per-row derived scalars projected with `=`: arithmetic over the
# numeric family, string concatenation (`||`), and a `case … when … then … else … end`
# conditional. Not aggregates: they compose in ordinary (non-group) shapes.
City { id: Id, name: text }

Product {
  id:       Id
  name:     text
  brand:    text
  price:    int
  discount: int
  rate:     decimal(12, 2)
  made_in:  City
}

shape ProductCard from Product {
  name
  net    = price - discount            # arithmetic -> int
  gross  = (price + discount) * rate   # promotes to decimal
  label  = brand || " " || name        # concatenation -> text
  tier   = case when price > 100 then "premium" else "standard" end
  origin = case when price > 0 then made_in.name else "unknown" end
}

query product_card(id) -> ProductCard;
