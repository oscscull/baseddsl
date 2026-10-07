scope Tenant (org: Org = $ctx.org)

@scope Tenant
@sort(id asc)
Order {
    id: Id
    org: Org
    total: int
    @index org
}

mutation place_order(total) -> Order guard caller_can_place scoped Tenant {
    create Order { total = $total };
}
query my_orders() -> Order[] scoped Tenant { list Order; }
