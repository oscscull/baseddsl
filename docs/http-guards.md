# HTTP guards in standalone services

A mutation declaring `guard permission` can run in `based serve` when an operator
maps `permission` to an authenticated callback. Embedded Rust guards continue to
use `Guards::register`; both doors enforce the same preflight check.

Create an operator-owned `guards.toml`:

```toml
[[guards]]
name = "permission"
endpoint = "https://policy.internal/check"
secret_env = "POLICY_CALLBACK_SECRET"
timeout_ms = 2000
```

Set `POLICY_CALLBACK_SECRET` in the service's **process environment**, then run:

```sh
based serve --guard-config /etc/based/guards.toml
```

`BASED_GUARD_CONFIG` may supply the path instead. Relative paths resolve against
the invocation directory. Secrets are loaded from process environment, not `.env`;
the file holds only secret variable names. Startup rejects missing, duplicate,
unknown and invalid mappings before opening databases or accepting requests.
Every declared guard needs a mapping. Optional `timeout_ms` defaults to 2000 and
must be 1–30000. For local development only, set `allow_loopback_http = true` for
`http://127.0.0.1`, `http://[::1]` or `http://localhost`. HTTPS always verifies
certificates and hostnames using Rustls with bundled Mozilla trust roots. Private
callbacks may add `ca_file = "policy-ca.pem"` to their mapping; it resolves relative
to the configuration file and must contain a PEM CA certificate. Trust roots are
loaded before serving; restart after CA changes. Verification cannot be disabled. URL credentials and fragments
are rejected. Network policy should restrict egress to the callback services.

The [v1 protocol](../spec/external-guards.md) defines the five-field POST envelope,
callback bearer authentication and exact allow/deny JSON. No caller headers are
forwarded. Callback URLs and secrets never come from mutation args or context.
Callbacks must authenticate before parsing policy data, validate args they use,
and return public denial text without secrets. Diagnostics identify the guard and
failure class and omit endpoints, credentials and response bodies.

The adapter has a total deadline, a 256 KiB request limit and 16 KiB response limit,
including chunked bodies. Redirects, retries, response decompression and environment
HTTP proxies are disabled. Dropping an invocation cancels its outbound future.
Invalid or unavailable callbacks return `403 guard_denied` with `Guard check
unavailable`; only a valid allow can reach normal mutation validation and execution.
An explicit denial forwards its public message. All denials precede writes and
idempotency claims. Replays check current permission again; internal transaction
retries do not invoke the callback again. Callbacks may still receive cancelled
requests and must tolerate repetition. Keep them read-only.

`based serve` still requires a trusted auth edge to authenticate users, strip client
`X-Based-*` headers and derive context. An authenticated callback is a separate
service credential, not an authenticator for the caller. Restrict direct listener
access. Callback reads, if needed, must use an authenticated query-only service
path with authorized scope; they do not share the write transaction. Keep state
invariants in database conditions, as in the
[embedded helpdesk guard example](../examples/axum-helpdesk/README.md).

The runtime API is available through the optional `external-guards` feature:
`HttpGuard::new(...).register(Guards::new(), name)` supplies a registry to
`http::serve_with_guards`, together with an explicit idempotency store. Embedded
consumers choosing Rust closures need no HTTP callback dependencies.
