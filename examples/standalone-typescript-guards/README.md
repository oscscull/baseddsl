# Standalone orders with TypeScript permission guards

This small trusted backend demonstrates the same Org/Order tenant domain as the
[embedded SQLite quickstart](../sqlite-quickstart/README.md). A TypeScript callback
checks permission while Based owns guard invocation and scoped database writes.
A separate auth edge derives tenant/user context from authenticated credentials.
There is no frontend, client generator or database server.

Prerequisites: Node.js 20 or later, npm, and a `based` CLI built with HTTP guard
support. From this directory:

```sh
npm ci
npm run demo
```

For a source checkout, build the CLI first with `cargo build -p based-cli`, then:

```sh
BASED_BIN="../../target/debug/based" npm run demo
```

The demo creates a disposable SQLite project, **explicitly runs** `based migrate
gen` and `based migrate apply`, and starts the callback, Based and auth edge on
loopback ports. It seeds one Org through an internal setup-only operation, then
uses `fetch` against the ordinary `/m/place_order` and `/q/my_orders` wire surface.
Only those two routes are exposed by the edge. On success it prints:

```text
buyer: allowed (200); viewer with forged context: denied (403)
forged callback: rejected (401); stopped callback: denied (403); persisted orders: 1
```

It also verifies a matching keyed retry returns the original order, unsigned
requests are rejected, the internal Org setup route is unavailable through the
edge, and the same replay is denied after the callback stops. All processes,
listeners and temporary data are cleaned up on exit. `npm test` runs this same
scenario; `make ci-typescript-guards` builds the CLI and runs it in CI.

The fixed `local-buyer-token` and `local-viewer-token` credentials are **local-only
examples**, not production authentication. `identity.ts` maps them to verified
identities and the seeded Org. The edge constructs fresh upstream headers and
ignores caller context/shard headers; a viewer cannot send a buyer verdict or
select the callback destination. Body fields remain ordinary mutation arguments.
Replace the credential lookup with your verified sessions and authorized tenant
membership, bind the internal services privately and enforce network restrictions.
Loopback on a shared host is not a security boundary between local users.

`auth.ts` performs callback credential comparison; `callback.ts` authenticates
before reading the bounded request. `policy.ts` separately validates the protocol
and values it uses and allows buyers to place a nonnegative integer order.
The callback secret is random per run and passed only through the process
environment; trusted `guards.toml` selects its environment variable and endpoint.
The HTTP loopback exception is explicitly development-only. Production callbacks
require certificate-verified HTTPS; see [HTTP guard configuration](../../docs/http-guards.md)
and the [v1 protocol](../../spec/external-guards.md).

This policy makes no callback database read and performs no external side effects.
If your policy needs a read, expose a restricted authenticated **query-only** service
path, derive its authorized tenant context on the server and bound its deadline and
results. Callback input must not become trusted context without authorization.
Reads do not share the mutation transaction. Keep state invariants in database
conditions/locks, as shown by the
[embedded Rust helpdesk guard](../axum-helpdesk/README.md); Rust closures remain
fully supported.

Guards may run repeatedly on user retries and replays; callback timeouts or
cancellation cannot undo a request already received. Keep decisions read-only and
repeatable. Missing mappings prevent startup; denied, stopped, malformed or
unauthenticated callbacks fail closed before writes or key claims.
The demo uses process-local memory replay via `BASED_IDEMPOTENCY_STORE=memory`;
restarting it loses keys. Production should use the
[durable database store](../../docs/standalone-idempotency.md), with explicit table
setup and retention planning. Neither store makes external effects exactly once.
