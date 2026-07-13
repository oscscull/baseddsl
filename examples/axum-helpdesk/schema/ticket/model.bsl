# The ticket — the desk's center of gravity.

# A string enum stores text + a CHECK; the variant NAME is what source and the
# wire client use, the VALUE is what the column stores (`waiting` reads clean in
# a query even though the stored value is the legacy "waiting_on_customer").
enum Status { open, waiting = "waiting_on_customer", resolved, closed }

# An int enum is ordered, so `priority >= high` works in a predicate.
enum Priority { low = 1, normal = 2, high = 3, urgent = 4 }

# Two stacked `@scope` lines are alternatives (OR): a callable confined by EITHER
# the tenant (agents see the whole org) or the requester (people see their own
# tickets) satisfies the contract. Cross-org access is inexpressible without a
# greppable `unscoped(...)`.
@soft_delete(deleted_at)
@created(created_at)
@updated(updated_at)
@sort(created_at desc)
@scope Tenant
@scope Requester
Ticket {
  deleted_at:   timestamp?
  created_at:   timestamp
  updated_at:   timestamp
  org:          Org
  requester:    User
  assignee:     User?
  subject:      text @was("title")
  status:       Status (default open)
  priority:     Priority (default normal)
  tags:         json?
  duplicate_of: Ticket?
  duplicates:   Ticket[]    (Ticket.duplicate_of)
  comments:     Comment[]   (Comment.ticket) @sort(created_at)
  time_entries: TimeEntry[] (TimeEntry.ticket)
  @index(org, status)
  @index assignee
  @index requester
}

# List row: bare local fields plus one reach-and-rename across the requester edge.
shape TicketRow from Ticket {
  id
  subject
  status
  priority
  created_at
  requester_name = requester.name
}

# Full detail. To-one edges nest by named-shape reference (`-> UserRef`); the
# to-many nests come back as arrays ordered by their traversal's sort — comments
# by the relation `@sort` above, time entries by TimeEntry's own model `@sort`.
shape TicketDetail from Ticket {
  id
  subject
  status
  priority
  tags
  created_at
  updated_at
  requester -> UserRef
  assignee -> UserRef
  duplicates -> TicketRow
  comments -> CommentRow
  time_entries { hours, amount, note, logged_at }
}

# One export line. `age_days` is a raw SQL leaf — the engine still owns the rest
# of the statement (scope, soft-delete, ordering); the raw text owns only itself.
shape TicketExport from Ticket {
  id
  subject
  status
  priority
  created_at
  org       = org.slug
  requester = requester.email
  age_days  = sql`extract(day from now() - created_at)::int`
}
