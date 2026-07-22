# A bearer token and what it resolves to. The auth middleware trades
# `Authorization: Bearer <token>` for this row once per request; everything the
# desk later trusts as `$ctx` is derived here, never taken from a request body.
@scope Tenant
Session {
  id:    Id
  org:   Org
  user:  User
  token: text
  @index token unique
}

# What a resolved token yields: the typed ids the request context is built from,
# plus the caller's role for route gating.
shape SessionCtx from Session {
  org  = org.id
  user = user.id
  role = user.role
}
