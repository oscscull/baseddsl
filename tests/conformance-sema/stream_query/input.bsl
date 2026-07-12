# A stream query resolves like its `[]` twin: verb `list`, same target/shape —
# only the delivery differs. Everything composes: filters, sort cascade, `$ctx`
# scope injection.
Org { name: text }
scope Tenant (org: Org = $ctx.org)

@scope Tenant
@sort(placed_at desc)
Order {
  org: Org
  status: text
  total: int
  placed_at: timestamp
  @index(org, placed_at)
}
shape OrderCard from Order { status, total }

query export_orders() -> stream OrderCard scoped Tenant;
query big_orders(min: int > total) -> stream OrderCard scoped Tenant;
