# BW2 — bulk upsert: `create Model[] from $rows on conflict (target) update { … }`. Reuses
# the singular upsert's conflict-target rules (D102) over the input shape's columns, plus the
# `incoming.<col>` incoming-row keyword (a contextual keyword, valid only here).
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)

@scope Tenant
Inventory {
  id: Id
  org: Org
  sku: text
  qty: int
  price: int
  @index(org, sku) unique
  @index(org)
}

shape InvIn from Inventory { sku, qty, price }
shape InvOut from Inventory { sku, qty, price }

# Valid: accumulate the stored qty with the incoming qty, take the incoming price. The
# conflict target is the composite unique key (scope col + sku). Reads back `-> InvOut[]`.
mutation restock(rows: InvIn[]) -> InvOut[] scoped Tenant {
  create Inventory[] from $rows
    on conflict (org, sku) update { qty = qty + incoming.qty, price = incoming.price };
}

# Valid: `-> ok` bulk upsert (no read-back).
mutation restock_ok(rows: InvIn[]) -> ok scoped Tenant {
  create Inventory[] from $rows on conflict (org, sku) update { qty = incoming.qty };
}

# `incoming.<col>` naming a non-column -> E0333.
mutation bad_incoming(rows: InvIn[]) -> ok scoped Tenant {
  create Inventory[] from $rows on conflict (org, sku) update { qty = incoming.nope };
}

# `incoming` outside a bulk `on conflict update` branch (an inline create assign) -> E0334.
mutation bad_context(sku: text, qty: int) -> ok scoped Tenant {
  create Inventory { sku = $sku, qty = incoming.qty, price = 0 };
}

# The conflict target is not a declared unique key (`price` alone) -> E0250.
mutation bad_target(rows: InvIn[]) -> ok scoped Tenant {
  create Inventory[] from $rows on conflict (price) update { qty = incoming.qty };
}

# A soft-delete model can't carry `on conflict` -> E0253.
@soft_delete(deleted_at)
@scope Tenant
Ledger {
  id: Id
  org: Org
  code: text
  hits: int
  deleted_at: timestamp?
  @index(org, code) unique
  @index(org)
}
shape LedgerIn from Ledger { code, hits }
mutation bump(rows: LedgerIn[]) -> ok scoped Tenant {
  create Ledger[] from $rows on conflict (org, code) update { hits = hits + incoming.hits };
}
