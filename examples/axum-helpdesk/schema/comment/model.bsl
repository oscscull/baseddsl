# The public conversation on a ticket. Soft-deleted (a redacted comment leaves a
# tombstone); truly removing one is the loud `hard delete` in comment/queries.bsl.
@soft_delete(deleted_at)
@scope Tenant
Comment {
  id:         Id
  deleted_at: timestamp?
  created_at: timestamp (default now())
  org:        Org
  ticket:     Ticket
  author:     User
  body:       text
  @index ticket
}

shape CommentRow from Comment {
  id
  body
  created_at
  author -> UserRef
}
