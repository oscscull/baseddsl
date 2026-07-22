# An agent's private working notes on a ticket. One `@scope` line with two names
# is a conjunction (AND): a draft is visible only inside its org AND to its
# author — every callable must confine by both axes.
@sort(created_at desc)
@scope Tenant, Author
DraftNote {
  id:         Id
  created_at: timestamp (default now())
  org:        Org
  author:     User
  ticket:     Ticket
  body:       text
  @index(org, author)
}

shape DraftRow from DraftNote {
  id
  body
  created_at
  ticket = ticket.id
}
