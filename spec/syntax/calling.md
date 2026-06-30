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
