query active_products(org: Id) -> ProductCard[] {
  list Product
    where (org = $org and active)
    order (created_at desc)
    page (20);
}
