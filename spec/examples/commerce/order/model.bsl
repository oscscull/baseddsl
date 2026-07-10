# model + its read contract live together; access layer is in queries.bsl

# The Tenant scope (auth.md Handle 2 / D46): a named row-visibility contract declared
# once, referenced by name on both sides — `@scope Tenant` on the model below, `scoped
# Tenant` on every callable that touches it (queries.bsl). The `org: Org` term is the one
# place the scope column's — and thus `$ctx.org`'s — type is written.
scope Tenant (org: Org = $ctx.org)

# An `enum` is a closed set of lowercase variants and a first-class scalar type: a field
# typed `Status` is a text column constrained to these values (a DB CHECK), not a relation.
enum Status { pending, paid, shipped, cancelled }

@soft_delete(deleted_at)
@sort(placed_at desc)
# Every read + write on Order is filtered to the caller's org, and a `create` auto-sets
# `org` from `$ctx` — cross-org access is inexpressible without the greppable
# `unscoped(...)` opt-out (see queries.bsl).
@scope Tenant
Order {
  deleted_at:   timestamp?
  org:          Org
  placed_by:    User
  fulfilled_by: User?
  status:       Status (default pending)
  total:        int
  placed_at:    timestamp (default now())
  items:        OrderItem[]
  @index(org, status)
  @index placed_at
  @index placed_by
}

shape OrderCard from Order {
  status
  total
  buyer = placed_by.name
  org   = org.name
}

# `placed_by -> UserRef` nests the buyer as the *named* `UserRef` projection
# (user/model.bsl) — the same rows an inline `placed_by { name, email }` nest fetches,
# but every query returning it shares the one nominal `UserRef` type.
shape OrderDetail from Order {
  status
  total
  placed_by -> UserDetail
}
