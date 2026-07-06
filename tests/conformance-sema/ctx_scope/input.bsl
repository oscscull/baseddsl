# `@scope` (D32): a uniform single-owner filter injected into every read + write, and
# auto-set on `create` from `$ctx` — so `place_order` takes no `org` (cross-scope create
# is inexpressible; assigning `org` would be E0181). Exercises per-callable $ctx inference
# (org typed as a relation to Org from the FK it maps to), including the create-time
# requirement the auto-set introduces.
@soft_delete(deleted_at)
Org {
  deleted_at: timestamp?
  name:       text
}

@soft_delete(deleted_at)
@scope(org = $ctx.org)
Order {
  deleted_at: timestamp?
  org:        Org
  total:      int
}

shape OrderCard from Order { total }

query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }

mutation place_order(total: int) -> OrderCard {
  create Order { total = $total };
}
