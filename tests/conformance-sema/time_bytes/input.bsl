# `time` and `bytes` scalar types (D117). `time` is a time-of-day, ordered like
# date/timestamp (`< > <= >=`); `bytes` is a binary blob, base64 on the wire and
# equality-only (`= != in`). Diagnostics: E0150 (ordered op on bytes), E0313 (a
# non-string `time` default), E0314 (a literal default on a `bytes` column).
Event {
  id:       Id
  start_at: time (default "09:00:00")
  payload:  bytes?
  @index (start_at)
  @index (payload)
}

# Clean: an ordered filter on a `time` column (allowed, like date/timestamp).
query after(t: time) -> Event[] {
  list Event where (start_at >= $t) order (start_at);
}

# Clean: equality on a `bytes` column.
query by_payload(p: bytes) -> Event[] {
  list Event where (payload = $p) order (start_at);
}

# E0150: an ordered comparison on a `bytes` column — bytes is not orderable.
query bad_cmp(p: bytes) -> Event[] {
  list Event where (payload > $p) order (start_at);
}

# E0313 (non-string time default) + E0314 (a bytes column can't carry a literal default).
BadDefaults {
  id:   Id
  t:    time (default 5)
  blob: bytes (default "AA==")
}
