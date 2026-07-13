# Both axes of the AND scope, spelled on every callable: `scoped Tenant` alone
# would be too few — the compiler rejects a partial confinement.

query my_drafts(ticket) -> DraftRow[] scoped Tenant, Author;

mutation save_draft(ticket: Id, body: text) -> DraftRow scoped Tenant, Author {
  create DraftNote { ticket = $ticket, body = $body };
}
