# Tenant onboarding. Org carries no `@scope`, so this needs no `$ctx` — the
# engine mints the id and hands it back typed.
mutation create_org(name, slug) -> OrgRow {
  create Org { name = $name, slug = $slug };
}
