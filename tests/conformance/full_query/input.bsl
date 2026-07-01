query active_products(org: Id, active: bool = true) -> ProductCard[] {
  list Product
    where (org = $org and active = $active)
    order (created_at desc)
    page (20);
}

filter recent = deleted_at = null and created_at > now();
