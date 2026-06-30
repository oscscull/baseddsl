# bare / per-param `->` / full-body query tiers, plus a mutation
query order_by_id(id) -> OrderCard;
query orders_in_org(org) -> OrderCard[];
query orders_by_buyer(user -> placed_by) -> OrderCard[];

mutation place_order(org: Id, buyer: Id) -> OrderCard {
  create Order { org = $org, placed_by = $buyer };
}
