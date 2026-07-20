# syntax/calling.md

Principles: 5 (closed query set = the access-pattern set the guarantees need), 1.

## Closed RPC, not open composition
Clients call pre-defined query/mutation signatures only. They never write or send the DSL. The wire carries arguments, not queries.

Closed is required: index inference, N+1 lint, soft-delete verification, equivalence-checking all depend on the query set being known at author time. Open composition forfeits all of them.

GraphQL's field-picking is moot (shape + filter fixed server-side) -> RPC-style surface, not GraphQL.

## Each signature generates
1. Typed client method — input type from params, return type from `-> Output`. Real codegen. e.g. `client.products({ org, active })` returns `{ rows: OrderCard[], cursor: string | null }`.
2. Wire endpoint — one route per query, e.g. `POST /q/products`, JSON body = typed inputs, returns typed output. No query string on the wire -> no injection surface, client can ask only what signatures pre-authorize.
3. Input validation — typed params validated at the boundary before SQL; defaults applied.

## Typed ids
On the typed surface, every id the client touches — a model's own `id`, a relation
param/FK, or a `$ctx` relation — is a per-entity phantom
newtype `Id<E>` (Rust: `Id<entity::User>`, `Id<entity::Org>`; a branded/named type in
other targets). Distinct entities are distinct types, so an `Org` id can never be passed
where a `User` id is wanted — the transposition that a shared string type type-checks and
fails only at runtime (an FK violation) is now a compile error. The compiler already
resolves which entity a param identifies (the same edge it type-checks); the client stops
discarding that and carries it to the language boundary — the one place a human/LLM writes
a call by hand.

The wire is unchanged: `Id<E>` is transparent over the underlying id string (no custom
serialization), so the OpenAPI/JSON surface is still `{ type: string, format: uuid }`. A
`create_*` result already hands back the typed id, so a create→use chain needs no
conversion. There is deliberately **no** blanket `From<String>` — that would reopen the
hole; turning a raw string into a typed id is an explicit, greppable `Id::from_raw(s)`
(mirroring how `unscoped(...)` makes the unsafe escape visible, principle 1/6).

## Acknowledgement mutations (`-> ok`)
A destructive mutation (`-> ok`, mutations.md) has no row to return: the wire success is `{}`, the
generated method returns **unit** — `purge_comment(input, ctx)` is `Result<(), ClientError>` — and
OpenAPI advertises the shared empty `Ack` object schema. Its keyed twin returns unit the same way.
A DELETE matching no row (absent, or another scope's) is the standard `404 not_found` error, so a
caller always gets a verdict, never an empty success for a miss.

## Pagination envelope
A query with `page (N)` returns `{ rows, cursor }`: the rows plus an opaque cursor for the next page. Codegen knows this from the body. Next page = same call + `cursor`; threading is generated, client never assembles keyset mechanics (pagination.md).

A `with count` query's envelope also carries `total` — the live-row count of the whole set. On the client, `Page<T>.total` is `Option<i64>`: `Some` exactly when the query declares `with count` (the wire has the field only then), `None` otherwise. OpenAPI advertises `total` (an `int64` integer) only on a `with count` query's page schema.

The cursor is a typed `Cursor` on the client surface — a `#[serde(transparent)]` newtype over the underlying string, so the wire stays an opaque cursor string and OpenAPI still describes it as `{ type: string }`. It is opaque by design: a page result hands one back and the caller feeds it straight to the next call, so a create→paginate→next-page chain needs no conversion. A single `Cursor` type covers every query (a cursor is not entity-typed the way an `Id<E>` is — it encodes a sort-key basis the runtime checksum-validates, cursor.rs). Turning a raw string into a `Cursor` is an explicit, greppable `Cursor::from_raw(s)` for the rare case a cursor arrives from outside the client.

## Idempotency keys
A mutation retried after a timeout risks running its write twice; an **idempotency key**
makes the engine run the body at most once per key and replay the first attempt's
recorded response (runtime semantics: same key + same payload → the recorded response;
same key + different payload → `422 idempotency_key_reuse`; a concurrent duplicate →
`409 idempotency_conflict`).

On the typed surface every mutation method has a keyed twin — `place_order(input, ctx)`
and `place_order_with_key(input, ctx, key)` — so the common no-key call stays clean and
the retry-safe call is one suffix. The key is **out-of-band request metadata, never an
input field**: an HTTP transport sends it as the standard `Idempotency-Key` header
(`Transport::call_with_key` carries it), and the embedded bridge hands it to
`Engine::call_with_key`. Both transports share the runtime's one dedupe path, so the
replay contract is identical in-process and over the wire. Like the streaming surface,
the keyed surface is emitted only when the schema declares a mutation — a query-only
schema's module is byte-identical to before.

## Streaming envelope
A `-> stream` query keeps its one route but answers with an NDJSON body, and its client
method returns a `Stream` of typed rows instead of a `Vec` (the `Transport` trait carries a
streaming call beside `call`; the embedded transport yields the engine's row stream
in-process). Wire framing, mid-stream error contract, cancellation: streaming.md.

## Transport + the embedded bridge (D62)
The generated `Client<T>` is generic over a `Transport` trait the module *defines itself* (post typed input + typed `$ctx` to a route, decode the reply). Both the trait's `call` and every client method are `async` — a transport awaits its round-trip (an HTTP client's socket, or the in-process engine's execution). A wire/HTTP transport is the caller's; the module carries no HTTP stack.

The in-process path is different: because the trait is defined in the generated module, the orphan rule forbids a library-side `impl Transport for Engine` in based-runtime — so `based gen client` **emits the bridge** when asked (`ClientOptions::embedded`). The module then also carries an `Embedded` transport over `based_runtime::Engine` and a one-call constructor:

```
let api = client::embedded(&engine);            // no bridge to write
let out = api.place_order(input, ctx).await?;   // typed, in-process, no socket
```

It is opt-in: the wire client leaves it off (a pure-wire consumer need not depend on based-runtime), an embedding build turns it on. based-codegen gains no based-runtime dep — the reference is by path in the emitted text, resolved by the consuming crate.

The generated module carries no inner `#![allow(dead_code)]` (an inner attribute breaks `include!`); include it under an outer `#[allow(dead_code)] mod client { … }`.

## Errors
Every client method returns `Result<Output, ClientError>`. `ClientError` is a real `std::error::Error` (so it chains with `?` and its cause is reachable via `source()`) with a `Display` and three accessors a caller can branch on without matching message text:
- `kind()` → `ClientErrorKind`: `Transport` (the round-trip never completed), `Decode` (a value would not (de)serialize), or `Api { status, code }` (the server ran the call and returned a structured error).
- `code()` → a stable machine string: the server's `error.code` for an api failure (e.g. `bad_arg`, `missing_ctx`, `database_error`), else `"transport"` / `"decode"`.
- `status()` → the HTTP status of an api failure (`None` for transport/decode).

The wire error envelope is `{ error: { code, message } }`; the embedded bridge and any HTTP transport rebuild a `ClientError` from it, preserving the server's status + stable code + message rather than flattening to an opaque string. The codes are those the runtime emits — `PlanError::code()` (boundary: `unknown_query`, `missing_arg`, `bad_arg`, `missing_ctx`, `bad_ctx`, `bad_cursor`, `internal`), `DbError::code()` (operational: `database_error`, `deadlock`, `pool_exhausted`), `not_found` (404 — a mutation's `where`, with its scope/soft-delete guards, matched no row: a surviving write's empty read-back or an `-> ok` DELETE affecting zero rows; nothing was written, mutations.md), the keyed-mutation outcomes (`idempotency_conflict` 409, `idempotency_key_reuse` 422), and `guard_denied` (403 — a Handle-3 guard rejected the call, auth.md) — a single source of truth shared by the wire and any in-process consumer.
