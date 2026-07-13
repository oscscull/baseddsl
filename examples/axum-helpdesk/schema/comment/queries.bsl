# The author is the caller — taken from the derived `$ctx`, never a body field.
# `org` is scope-managed on create.
mutation add_comment(ticket: Id, body: text) -> CommentRow scoped Tenant {
  create Comment { ticket = $ticket, author = $ctx.user, body = $body };
}

# Legal/PII removal is the one place a comment really dies. `hard delete` is the
# explicit opt-out of the soft action; there is no surviving row to read back.
mutation purge_comment(id: Id) -> CommentRow scoped Tenant {
  hard delete Comment where (id = $id);
}
