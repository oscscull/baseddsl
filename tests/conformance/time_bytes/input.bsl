# `time` (time-of-day) and `bytes` (binary blob) scalar primitives parse in type
# position like any other primitive, including a `time` string default.
Event {
  id:       Id
  start_at: time (default "09:00:00")
  ends_at:  time?
  payload:  bytes?
}
