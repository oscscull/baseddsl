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

## Pagination envelope
A query with `page (N)` returns `{ rows, cursor }`, never a bare array (cursor must come back). Codegen knows this from the body. Next page = same call + `cursor`; threading is generated, client never assembles keyset mechanics (pagination.md).

## Transport + the embedded bridge (D62)
The generated `Client<T>` is generic over a `Transport` trait the module *defines itself* (post typed input + typed `$ctx` to a route, decode the reply). A wire/HTTP transport is the caller's; the module carries no HTTP stack.

The in-process path is different: because the trait is defined in the generated module, the orphan rule forbids a library-side `impl Transport for Engine` in based-runtime — so `based gen client` **emits the bridge** when asked (`ClientOptions::embedded`). The module then also carries an `Embedded` transport over `based_runtime::Engine` and a one-call constructor:

```
let api = client::embedded(&engine);      // no bridge to write
let out = api.place_order(input, ctx)?;   // typed, in-process, no socket
```

It is opt-in: the wire client leaves it off (a pure-wire consumer need not depend on based-runtime), an embedding build turns it on. based-codegen gains no based-runtime dep — the reference is by path in the emitted text, resolved by the consuming crate.

The generated module carries no inner `#![allow(dead_code)]` (an inner attribute breaks `include!`); include it under an outer `#[allow(dead_code)] mod client { … }`.
