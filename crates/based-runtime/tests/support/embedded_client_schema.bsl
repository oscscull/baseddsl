@soft_delete(deleted_at)
Org { deleted_at: timestamp?, name: text }

@soft_delete(deleted_at)
@sort(total desc)
Order {
    deleted_at: timestamp?,
    org: Org,
    status: text,
    total: int,
    @index(org)
}
shape OrderCard from Order { status, total }

query order_by_id(id) -> OrderCard;
query orders_in_org(org) -> OrderCard[];
query export_orders(org) -> stream OrderCard;
query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }

mutation place_order(org: Id, status, total: int) -> OrderCard {
    create Order { org = $org, status = $status, total = $total };
}
