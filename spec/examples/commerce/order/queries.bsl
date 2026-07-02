# bare / per-param `->` / full-body query tiers, plus a mutation
query order_by_id(id) -> OrderCard;
query orders_in_org(org) -> OrderCard[];
query orders_by_buyer(user -> placed_by) -> OrderCard[];

# auth.md Handle 1: the caller supplies the acting org as request context; the
# query consumes it as `$ctx.org` (typed by `[ctx]` in based.toml, D4/D5). Served
# by `@index(org, status)`.
query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }

mutation place_order(org: Id, buyer: Id, total: int) -> OrderCard {
  create Order { org = $org, placed_by = $buyer, total = $total };
}
