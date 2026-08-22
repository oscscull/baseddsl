# Whole-table wipe (`delete all` / `hard delete all`): the required `all` keyword
# means every row (in scope). A hard wipe is a real DELETE / TRUNCATE; a soft-model
# `delete all` tombstones every row. All are `-> ok` (no single row reads back).
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)

Widget { id: Id, name: text }

@soft_delete(deleted_at)
Order { id: Id, deleted_at: timestamp? }

@scope Tenant
Invoice { id: Id, org: Org, @index(org) }

mutation wipe_widgets() -> ok {
  hard delete all Widget;
}

mutation archive_all_orders() -> ok {
  delete all Order;
}

mutation wipe_my_invoices() -> ok scoped Tenant {
  hard delete all Invoice;
}
