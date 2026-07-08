@soft_delete(deleted_at)
Org {
  deleted_at: timestamp?
  name:       text
  slug:       text (unique)
}

# What a create hands back — its id (so the caller can act as this tenant) + its fields.
shape OrgRow from Org { id, name, slug }

# Seed a tenant. Public: Org carries no `@scope`, so this needs no `$ctx`. The engine
# generates the id (uuid in prod, a sequential id under the demo's `SeqIdGen`).
mutation create_org(name, slug) -> OrgRow {
  create Org { name = $name, slug = $slug };
}
