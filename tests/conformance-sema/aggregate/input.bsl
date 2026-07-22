# Aggregations + group by + having: an aggregate shape (count/sum/avg/min/max)
# projected over groups. Clean when every non-aggregate column is grouped; a
# global (no group by) all-aggregate shape is one whole-table row. Errors:
# E0241 (sum over text), E0242 (ungrouped projected column), E0244 (page).
Buyer {
  id: Id
  name: text
}

@soft_delete(deleted_at)
Order {
  id: Id
  deleted_at: timestamp?
  buyer:      Buyer
  total:      decimal(12, 2)
  qty:        int
  note:       text
}

shape BuyerStats from Order {
  who     = buyer
  orders  = count()
  revenue = sum(total)
  avg_qty = avg(qty)
  biggest = max(total)
}

query buyer_stats() -> BuyerStats[] {
  list Order group by (buyer) having (revenue > 100) order (revenue desc);
}

shape OrderTotals from Order {
  orders  = count()
  revenue = sum(total)
}

query order_totals() -> OrderTotals {
  get Order;
}

shape SumText from Order {
  who = buyer
  x   = sum(note)
}

query bad_operand() -> SumText[] {
  list Order group by (buyer);
}

shape Ungrouped from Order {
  who    = buyer
  note
  orders = count()
}

query bad_grouping() -> Ungrouped[] {
  list Order group by (buyer);
}

query bad_page() -> BuyerStats[] {
  list Order group by (buyer) page (20);
}
