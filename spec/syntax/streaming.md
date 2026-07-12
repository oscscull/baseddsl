# syntax/streaming.md

Principles: 1, 2 (streaming is a visible contract, never inferred), 5, 7.

Streaming = the export / large-scan read: rows delivered one at a time, memory bounded by
a row, not the result set. Keyset pagination (pagination.md) stays the random-access answer
for UIs; streaming is the full forward pass.

## Opt-in: on the signature, by the author
A third return form: `-> stream Shape` in place of `-> Shape[]`.
```
query export_orders(org) -> stream OrderCard scoped Tenant;
query audit_trail(since: timestamp > created_at) -> stream EventRow;
```
The signature is the contract (queries.md) and streaming changes it — the client method's
return type and the wire body are different — so it lives where the contract lives, visible
in the signature. Not inferred from a size threshold (a contract that flips on data volume
is a consequence hidden by omission), not a caller-side flag (closed RPC: one signature =
one method = one wire shape). A query wanted both ways is two declared queries.

- `stream` already means many — no `[]` after the shape.
- Body verb is `list` (a stream is a list delivered incrementally); `get` is a cardinality
  mismatch (`E0200`). The bare form infers `list` as usual.
- Mutations never stream (`E0202`).

## What composes, what is forbidden
- **Filters, param bindings, named filters** — unchanged.
- **Sorting** — unchanged: the same cascade (query `order` > relation `@sort` > model
  `@sort`) and the same nondeterministic-order lint when no tier provides one.
- **Scope, soft-delete, `$ctx`** — unchanged; see "Nothing is bypassed".
- **Index lint** — unchanged: a deliberate full-scan export writes its `unindexed(...)`
  like any other query.
- **`page` is forbidden (`E0201`)** — a page is a bounded chunk plus a re-entry cursor;
  a stream is one unbounded forward pass. The envelopes contradict; a query has one.
  Paginate for random access, stream for the full pass.

## Per streamed row
Each streamed item is exactly one element of what the `[]` form's `rows` array would be —
same shape language, nests included, nothing restricted. A to-many nest materializes
**inside its row**: every item arrives complete. So streaming bounds how many rows are held
at once (one), not how wide a row is — a shape nesting an unbounded to-many still builds
that child array per row. If a child list can be huge, give the stream a trimmed shape.

## Wire: NDJSON
Same route (`POST /q/<name>`). Response: `200`, `Content-Type: application/x-ndjson`.
Every line is one single-key envelope object:
```
{"row":{...}}                                          # one per row, in sort order
{"done":{"rows":17}}                                   # terminal line on success
{"error":{"code":"database_error","message":"..."}}    # terminal line on mid-stream failure
```
- Errors before the body starts — unknown query, bad args, missing/bad `$ctx`, scope
  rejection — are the ordinary JSON error response with a real HTTP status. The stream
  begins only after validation and planning succeed.
- Once the body has begun the status line is spent; a database failure mid-stream arrives
  as the terminal `error` line (the same `{code, message}` envelope as everywhere) and the
  body ends.
- **No terminal line = failure.** A body ending without `done` or `error` was truncated
  (connection cut, server death); a client must treat it as a transport error, never as
  completion. `done` is the only success signal; its `rows` count doubles as a checksum.

Why NDJSON over a chunked JSON array: every line parses standalone — no incremental JSON
parser, `curl … | jq` works line by line — and a truncated array is indistinguishable from
a completed one short of a byte-level diff, while NDJSON gives the error and the success
signal a place to live in-band.

## Generated client
Same method name — one signature, one method (calling.md). The return type is the stream:
```rust
let mut rows = api.export_orders(input, ctx).await?;   // Err here: the call never started
while let Some(row) = rows.next().await {
    let order: OrderCard = row?;                        // Err here: mid-stream failure
}
```
- Outer `Result`: transport failure or a pre-body rejection (validation, scope, `$ctx`) —
  the same structured `ClientError` as any call.
- The stream yields `Result<Shape, ClientError>` per item. An in-band `error` line
  surfaces as an `Err` item carrying the server's stable code; a truncated body surfaces
  as a transport-kind `Err`. After an `Err` item the stream is finished.
- **Drop = cancel.** Dropping the stream mid-pass abandons the read and releases its
  connection; reads hold no transaction, so there is nothing else to unwind.
- `Transport` gains a streaming call beside `call`: an HTTP transport parses NDJSON lines;
  the emitted `Embedded` transport yields the engine's row stream in-process — same typed
  items, no socket.
- OpenAPI describes the response as `application/x-ndjson` whose line schema is the
  row/done/error envelope over the shape.

## Nothing is bypassed
A stream query is the same lowered SQL on the same execution path — the one-shot response
is that stream collected. Scope acknowledgement is still mandatory on a scoped model
(`scoped` / `unscoped`, queries.md), soft-delete filtering is injected identically, and
`$ctx` is validated before the first row. A streamed read can never see a row the `[]`
form would not.
