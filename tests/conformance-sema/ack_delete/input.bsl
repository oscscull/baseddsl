# A destructive mutation returns `ok`: the primary model is the first real
# DELETE's, scope still applies, and there is no return shape.
Org { name: text }
scope Tenant (org: Org = $ctx.org)

@soft_delete(deleted_at)
@scope Tenant
Comment {
  deleted_at: timestamp?
  org:        Org
  body:       text
}

mutation purge_comment(id: Id) -> ok scoped Tenant {
  hard delete Comment where (id = $id);
}
