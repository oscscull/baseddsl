@soft_delete(deleted_at)
Org { id: Id, deleted_at: timestamp?, name: text }

@soft_delete(deleted_at)
@sort(total desc)
Order {
  id: Id
    deleted_at: timestamp?,
    org: Org,
    status: text,
    total: int,
    @index(org)
    @index(status)
}
shape OrderCard from Order { status, total }

query order_by_id(id) -> OrderCard;
query find_orders(status?) -> OrderCard[];
query order_for_update(id) -> OrderCard { get Order where (id = $id) for update; }
query orders_in_org(org) -> OrderCard[];
query export_orders(org) -> stream OrderCard;
query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }
query order_page(org) -> OrderCard[] { list Order where (org = $org) page (2); }
query counted_order_page(org) -> OrderCard[] {
    list Order where (org = $org) page (2) offset with count;
}

mutation place_order(org: Id, status, total: int) -> OrderCard {
    create Order { org = $org, status = $status, total = $total };
}

mutation purge_order(id: Id) -> ok {
    hard delete Order where (id = $id);
}

Feature { id: Id, enabled: bool, beta: bool? }
shape FeatureView from Feature { enabled, beta }
query feature_by_id(id) -> FeatureView;
