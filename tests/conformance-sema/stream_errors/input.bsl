# The three stream misuses: a `get` body (E0200 — a stream is many rows), a
# `page` clause (E0201 — the envelopes contradict), and a mutation return
# (E0202 — a write returns its row once).
Order { id: Id, status: text, total: int }
shape OrderCard from Order { status, total }

query one_order(id) -> stream OrderCard {
  get Order where (id = $id);
}

query paged_orders() -> stream OrderCard {
  list Order
    order (total desc)
    page (20);
}

mutation place_order(status) -> stream OrderCard {
  create Order { status = $status, total = 0 };
}
