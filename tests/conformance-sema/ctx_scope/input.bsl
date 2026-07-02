# `@scope` (predicate over $ctx) + a query reading $ctx.org — exercises per-
# callable $ctx inference (org typed as a relation to Org from the FK it maps to)
# and a mutation whose ctx bag is threaded through its create.
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

mutation place_order(org: Id, total: int) -> OrderCard {
  create Order { org = $org, total = $total };
}
