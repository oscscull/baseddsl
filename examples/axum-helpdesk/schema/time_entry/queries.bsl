mutation log_time(ticket: Id, hours: float, amount: decimal(10, 2), note: text, logged_at: timestamp) -> TimeEntryRow {
  create TimeEntry { ticket = $ticket, agent = $ctx.user, hours = $hours, amount = $amount, note = $note, logged_at = $logged_at };
}

# The workload report wants aggregation, which the DSL doesn't model yet — so
# each rollup is a raw correlated-subquery leaf. Raw stays at the leaves: the
# engine still owns the row set (which agents, in which order); the raw text
# owns its own tombstone filter, which is why the `deleted_at` check is written
# out by hand.
shape AgentWorkload from User {
  name
  rate
  open_tickets = raw`(select count(*) from ticket t
                      where t.assignee_id = {id} and t.deleted_at is null
                        and t.status not in ('resolved', 'closed'))`
  hours_logged = raw`(select coalesce(sum(e.hours), 0) from time_entry e
                      where e.agent_id = {id})`
  billed       = raw`(select coalesce(sum(e.amount), 0) from time_entry e
                      where e.agent_id = {id})`
}

query workload_report(org) -> AgentWorkload[] { list User where (org = $org and role = agent); }
