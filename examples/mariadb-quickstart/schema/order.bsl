# The Tenant scope (auth.md Handle 2 / D46): a named row-visibility contract declared
# once and referenced by name on both sides — `@scope Tenant` on the model, `scoped
# Tenant` on every callable that touches it (queries.bsl). The `org: Org` term is the
# one place the scope column's — and thus `$ctx.org`'s — type is written.
scope Tenant (org: Org = $ctx.org)

# Every read + write on Order is filtered to the caller's org, and a `create` auto-sets
# `org` from `$ctx` — cross-org access is inexpressible without a greppable `unscoped(...)`.
@soft_delete(deleted_at)
@sort(placed_at desc)
@scope Tenant
Order {
  id:         Id
  deleted_at: timestamp?
  org:        Org
  placed_by:  User
  status:     text (default "pending")
  total:      decimal(12, 2)
  placed_at:  timestamp (default now())
  @index(org, status)
  @index placed_at
}

# The declared read shape. `placed_by { … }` nests the related User as a sub-object
# (L1/D55) — the response carries a real `placed_by: { name, email }`, not a flat id.
shape OrderCard from Order {
  id
  status
  total
  placed_by { name, email }
}
