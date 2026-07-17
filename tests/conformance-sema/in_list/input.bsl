# `in` value-list membership: variants + a `$param` element check clean against
# the enum column; a borrowed variant is E0154, a text element in a numeric
# column's list is E0151.
enum Status { open, waiting = "waiting_on_customer", closed }
enum Priority { low = 1, high = 2 }

Ticket {
  status:   Status
  priority: Priority
  total:    int
  @index(status)
  @index(total)
}

shape TicketRow from Ticket { status, total }

query active(extra: Status) -> TicketRow[] {
  list Ticket where (status in (open, waiting, $extra)) order (total);
}

query bad_variant() -> TicketRow[] {
  list Ticket where (status in (open, low)) order (total);
}

query bad_family() -> TicketRow[] {
  list Ticket where (total in (1, "two")) order (total);
}
