# Permission guards

A mutation declares a named host decision:

```bsl
mutation refund(id) -> RefundResult guard caller_can_refund scoped Tenant { ... }
```

Embedded applications register the name through `Guards`. For standalone
execution, configure an authenticated callback before starting the service:

```toml
[guards.caller_can_refund]
endpoint = "https://permissions.internal/check"
secret_env = "REFUND_GUARD_SECRET"
deadline_ms = 2000
```

```sh
based serve --guard-config guards.toml
```

Missing mappings refuse startup. HTTPS verifies certificates and hostnames;
`--guard-allow-loopback-http` permits HTTP only for local development.
Callbacks receive a POST with `Authorization: Bearer <secret>` and this v1 body:

```json
{"version":1,"guard":"caller_can_refund","callable":"refund","args":{"id":"..."},"ctx":{"org":"..."}}
```

An allowed response is `{"version":1,"verdict":"allow"}`. Denial uses
`"verdict":"deny"` and can include `"message"`. Invalid, unavailable,
oversized, or timed-out callbacks deny the mutation with `403 guard_denied`.
The adapter does not follow redirects or retry callbacks. Guards run before database
checkout and idempotency claims, including on replay. Internal transaction retries
reuse that invocation's decision.

Keep callbacks to permission decisions. Put race-sensitive invariants in atomic
write conditions or transactions; callback side effects are not part of a database
commit. The host or trusted edge must derive the context.

## Example

From the repository root, run the TypeScript orders example:

```sh
cargo build -p based-cli --no-default-features
cd examples/standalone-typescript-guards
npm ci
BASED_BIN=../../target/debug/based npm test
```

Read its authentication edge, callback, and BSL before adapting it. The edge uses
local example tokens, constructs fresh forwarding headers, and keeps the backend
and callback on loopback. Production deployment needs your own identity system
and [network boundary](reference.md).
