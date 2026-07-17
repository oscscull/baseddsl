# Ticket access, by audience. Ticket is scoped (ticket/model.bsl), so every
# callable names the alternative that confines it — `scoped Tenant` for the
# agent desk, `scoped Requester` for the portal — or opts out with a reason.

# Open means "not settled" — excluding the two terminal states keeps this true
# if a new active status is ever added. `in` takes a value list; each variant is
# membership-checked against Status.
filter open_states = not status in (resolved, closed);

# Raw predicate leaf: interval math the DSL doesn't model. It composes as one
# boolean term; scope and soft-delete still wrap the query around it.
filter overdue = sql`created_at < now() - interval '3 days'`;

# ---- requester portal ------------------------------------------------------

query my_tickets() -> TicketRow[] scoped Requester { list Ticket; }

# One transaction: the ticket and its first comment land together or not at all.
# `^` reads the row the preceding create produced. The scope columns (`org`,
# `requester`) are engine-set from `$ctx` — a caller can't file into another
# tenant. Retries ride the client's `open_ticket_with_key` twin.
mutation open_ticket(subject: text, body: text, priority: Priority = normal) -> TicketDetail scoped Tenant, Requester {
  tx {
    create Ticket { subject = $subject, priority = $priority };
    create Comment { ticket = ^.id, author = $ctx.user, body = $body };
  }
}

# ---- agent desk ------------------------------------------------------------

query ticket(id) -> TicketDetail scoped Tenant;

# Per-param bindings: `agent -> assignee` binds via the named edge, `since`
# binds with an explicit column + operator (`created_at > $since`).
query tickets_for(agent -> assignee, since: timestamp > created_at) -> TicketRow[] scoped Tenant;

query search_tickets(q: text, status: Status = open) -> TicketRow[] scoped Tenant {
  list Ticket
    where (subject ~ $q and status = $status)
    order (priority desc, created_at desc)
    page (20);
}

# My open work: urgent by rank, or anything that has sat too long. `$ctx.user`
# is a filter value the caller's auth layer produced, not a decision.
query queue() -> TicketRow[] scoped Tenant { list Ticket where (assignee = $ctx.user and open_states and (priority >= high or overdue)); }

# JSON containment: `tags has $tag` — pass a JSON value, e.g. "vip".
query tagged_tickets(tag: json has tags) -> TicketRow[] scoped Tenant;

mutation assign_ticket(id: Id, agent: Id) -> TicketRow scoped Tenant {
  update Ticket where (id = $id) { assignee = $agent };
}

# Triage: enum values are assigned by variant name, tags as a JSON array.
mutation set_status(id: Id, status: Status) -> TicketRow scoped Tenant {
  update Ticket where (id = $id) { status = $status };
}

mutation tag_ticket(id: Id, tags: json) -> TicketRow scoped Tenant {
  update Ticket where (id = $id) { tags = $tags };
}

mutation mark_duplicate(id: Id, of: Id) -> TicketRow scoped Tenant {
  update Ticket where (id = $id) { duplicate_of = $of };
}

# `guard` hands the close decision to a host-language function the app registers
# at engine build; the engine owns that it runs before the write, on every door.
mutation close_ticket(id: Id) -> TicketRow guard caller_can_close scoped Tenant {
  update Ticket where (id = $id) { status = closed };
}

# `delete` on a soft-delete model tombstones; `restore` lifts it. Both read the
# row back in the declared shape.
mutation archive_ticket(id: Id) -> TicketRow scoped Tenant {
  delete Ticket where (id = $id);
}

mutation restore_ticket(id: Id) -> TicketRow scoped Tenant {
  restore Ticket where (id = $id);
}

# ---- ops / finance ---------------------------------------------------------

# Cross-tenant support view: offset pagination + a total, for a paged admin table.
# Ops-only traffic, so the full-table sort is acknowledged rather than indexed.
query admin_tickets() -> TicketRow[] unscoped("ops: cross-tenant support view") {
  list Ticket
    order (created_at desc)
    page (50) offset with count
    unindexed(unsafe, "ops-only view; a paged scan is fine");
}

# The compliance export: one unbounded forward pass, a row at a time — NDJSON on
# the wire, a typed `Stream` on the client. Deliberately unindexed: it reads
# nearly everything anyway.
query export_tickets(since: timestamp > created_at) -> stream TicketExport unscoped("compliance: whole-desk retention export") unindexed(unsafe, "full-scan export; runs off-peak");
