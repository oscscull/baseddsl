# The tenant. Every scoped row in the desk hangs off one Org.

# The Tenant scope: a named row-visibility contract, declared once and referenced
# by name on both sides — `@scope Tenant` on a model, `scoped Tenant` on every
# callable that touches it. The `org: Org` term is the one place the scope
# column's — and thus `$ctx.org`'s — type is written.
scope Tenant (org: Org = $ctx.org)

Org {
  id:   Id
  name: text
  slug: text (unique)
}

shape OrgRow from Org { id, name, slug }
