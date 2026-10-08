# Standalone external guards: protocol v1

This is the normative contract for the HTTP adapter configured through
[`based serve --guard-config`](../docs/standalone-guards.md). Embedded Rust
closures remain supported through `Guards`. Both implementations use the
same [preflight enforcement semantics](syntax/auth.md#handle-3--guard-hook-into-caller-code).

## Trusted configuration and startup

An operator maps each declared BSL `guard` name to one endpoint, one callback
bearer secret, and a deadline. Validate all mappings before accepting requests:
missing mappings, empty secrets, invalid URLs or limits prevent startup and name
the affected guard. URLs must use HTTPS with certificate and hostname verification.
Reject URL credentials, fragments, and redirects. The explicit development-only
HTTP exception permits literal loopback hosts (`127.0.0.1`, `[::1]`, `localhost`);
it must be enabled by the operator and is never inferred from request headers.
Do not disable TLS verification to support local development.

Endpoints, secrets, limits and the local exception come only from trusted server
configuration. Request args, context, shard headers, and caller-supplied headers
cannot select or override them. Deployment operators restrict outbound network
access to their callback services. This protocol does not make arbitrary URLs
safe for untrusted tenants to configure.

## Request and response

Send one `POST` per guard invocation with `Content-Type: application/json`,
`Accept: application/json`, and `Authorization: Bearer <configured secret>`.
Never forward the incoming HTTP authorization or `X-Based-*` headers. The
callback authenticates this credential before examining or acting on the body;
invalid credentials produce a non-200 response. Rotate secrets through operator
configuration. Neither side logs credentials, raw args/context, or callback
response bodies; operational diagnostics may identify the guard and failure class.

```json
{
  "version": 1,
  "guard": "caller_can_close",
  "callable": "close_ticket",
  "args": { "id": "ticket-id" },
  "ctx": { "org": "trusted-org", "user": "trusted-user" }
}
```

`guard` is the declared name; `callable` is the mutation name. `args` is the
decoded request argument object, before callable argument validation, so the
callback must validate any values it uses. `ctx` is an object derived by the
trusted auth edge/application, not a permission verdict supplied by the client.
No engine handle, database URL, callback credential, or idempotency key is
serialized into the body. Only the five shown fields are sent in v1.

A successful transport response is HTTP 200 with JSON content type and exactly
one of these objects (no additional fields):

```json
{ "version": 1, "verdict": "allow" }
```

```json
{ "version": 1, "verdict": "deny", "message": "Permission denied" }
```

`version` must be integer 1 and `verdict` must be exactly `allow` or `deny`.
Only `deny` carries `message`: a nonempty, non-whitespace string, at most 1024
UTF-8 bytes. This is deliberately public text, forwarded to the mutation caller;
it must contain no secrets or internal policy details. Reject duplicate JSON keys,
trailing data, additional fields, absent/wrongly typed fields and unknown versions
or verdicts. An allow response carrying a message is invalid.

## Bounds and fail-closed behavior

The default total deadline is 2 seconds; an operator may select a positive
deadline up to 30 seconds. It covers connecting, TLS negotiation, sending, and
reading the complete response. The encoded request is limited to 256 KiB and
the response to 16 KiB, including chunked/unknown-length bodies. Reject an
oversized request before sending it. Disable response decompression and reject
non-identity content encoding so the response limit is unambiguous. Drop/cancel
the outbound future when the mutation request is cancelled. No automatic
callback retries or redirect following are permitted.

| Outcome | Public mutation result | Database effects |
| --- | --- | --- |
| Valid allow | Continue through normal validation, replay lookup and write execution | Only the ordinary mutation path may write |
| Valid deny | `403 guard_denied`, public message from the callback | No write or key claim |
| Timeout, DNS/connect/TLS failure, disconnect, non-200 (including auth failure), redirect, wrong content type/encoding, oversized body, invalid JSON/version/verdict | `403 guard_denied`, fixed message `Guard check unavailable` | No write or key claim |
| Missing registration reaching raw dispatch | Existing `500 guard_unregistered` backstop | No write |

Using the existing denial category keeps the adapter compatible with
`GuardVerdict`; callback failures never become an allow. Internal diagnostics
distinguish transport/timeout/protocol failures but expose no URL credentials or
callback internals to callers. Request cancellation returns no verdict and must
not start the mutation. Startup errors are configuration failures, not this table's
request-time failures.

## Replays, retries and reads

Dispatch checks the guard before argument/context validation, before any
idempotency lookup/claim, and before checking out the mutation transaction.
Consequently a replay request checks current permission again; denial prevents
even a stored response from being returned. The guard runs once per dispatch,
not once per internal deadlock/serialization transaction attempt. A caller retry,
including after a lost response, invokes it again. A callback may have received a
request even if Based timed out or cancelled; cancellation does not undo remote
work. Callbacks must tolerate repeated calls and should be read-only: they are
not an exactly-once external side-effect mechanism.

A callback that needs data may call a dedicated Based **query** through an
authenticated, restricted service path. That path verifies service credentials,
derives authorized tenant/user context, strips caller-provided context/shard
headers, and limits the callback identity to the required scoped queries. Do not
expose the listener directly or let callback body values become trusted context
without authorization. Query-only access avoids mutation-to-guard recursion;
callbacks must not invoke mutations or recursively call their own guard endpoint.
Bound read time by the callback deadline and bound query results separately.

These reads use a separate connection and do not join the mutation transaction.
Approval provides preflight permission, not an atomic read-check-write invariant.
Enforce state invariants with conditions/locks in the database transaction, as in
the [conditional helpdesk close](syntax/auth.md#preflight-permissions-and-atomic-invariants).

## Adapter verification contract

The implementation must exercise startup with missing/invalid mappings; one
authenticated allow and deny; forged credentials; every failure class above;
request and streamed-response size bounds; cancellation; and certificate/hostname
verification against controlled local servers. Assert zero persisted writes and
no consumed key on denial/failure. Existing embedded guard tests remain required.
Prove replays recheck permission and internal transaction retries do not resend
the callback. The TypeScript example in #76 must implement this same contract.
