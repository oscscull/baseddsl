# A genuinely keyless legacy table: `@no_id("reason")` opts out of the primary key
# (the reason is mandatory). It forfeits id-keyed operations — a `get`, a keyset page,
# and the create read-back all key on a `(unique)` column instead of a surrogate id.
@no_id("append-only audit log keyed by its natural `source`, no surrogate id")
AuditEvent {
  source: text (unique)
  action: text
  at:     timestamp
}

shape EventRow from AuditEvent { source, action, at }

query event_by_source(source) -> EventRow;
query recent_events() -> EventRow[] { list AuditEvent order (source) page (50); }

mutation record_event(source: text, action: text, at: timestamp) -> EventRow {
  create AuditEvent { source = $source, action = $action, at = $at };
}
