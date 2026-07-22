# Named scope (D46): a `scope` decl referenced by `@scope Tenant` on the model and
# `scoped Tenant` on every callable that touches it. The scope is injected into every
# read + write, and auto-set on `create` from `$ctx` — so `place_order` takes no `org`
# (cross-scope create is inexpressible; assigning `org` would be E0181). The `org: Org`
# term is the one place `$ctx.org`'s type is declared, sourcing it structurally instead
# of per-callable inference.
scope Tenant (org: Org = $ctx.org)

@soft_delete(deleted_at)
Org {
  id: Id
  deleted_at: timestamp?
  name:       text
}

@soft_delete(deleted_at)
@scope Tenant
Order {
  id: Id
  deleted_at: timestamp?
  org:        Org
  total:      int
}

shape OrderCard from Order { total }

query my_org_orders() -> OrderCard[] scoped Tenant { list Order where (org = $ctx.org); }

mutation place_order(total: int) -> OrderCard scoped Tenant {
  create Order { total = $total };
}
