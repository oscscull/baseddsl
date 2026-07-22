# Billable work on a ticket. `hours` is a measurement (float is fine); `amount`
# is money (decimal is exact — it rides the wire as a string, never a float).
# `logged_at` is when the work happened, supplied by the agent — so a ticket's
# time entries sort by this model `@sort`, not by insertion order.
@sort(logged_at)
TimeEntry {
  id:        Id
  ticket:    Ticket
  agent:     User
  hours:     float
  amount:    decimal(10, 2)
  note:      text?
  logged_at: timestamp
  @index ticket
}

shape TimeEntryRow from TimeEntry {
  id
  hours
  amount
  note
  logged_at
}
