# The author is the caller — taken from the derived `$ctx`, never a body field.
# `org` is scope-managed on create.
mutation add_comment(ticket: Id, body: text) -> CommentRow scoped Tenant {
  create Comment { ticket = $ticket, author = $ctx.user, body = $body };
}

# Legal/PII removal is the one place a comment really dies. `hard delete` is the
# explicit opt-out of the soft action; no row survives to read back, so the
# mutation returns the bare acknowledgement (`ok`) — a missing or cross-tenant
# id is a 404, never an empty success.
mutation purge_comment(id: Id) -> ok scoped Tenant {
  hard delete Comment where (id = $id);
}
