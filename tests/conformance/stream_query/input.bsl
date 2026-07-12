# The `-> stream Shape` return form: bare, inline, and block bodies all parse;
# `stream` is contextual (return-type position only), and it already means many —
# no `[]` after the shape.
Org { name: text }
Order { org: Org, status: text, total: int }
shape OrderCard from Order { status, total }

query export_orders(org) -> stream OrderCard;
query recent_orders(org) -> stream OrderCard order (total desc);
query all_orders() -> stream OrderCard {
  list Order;
}
