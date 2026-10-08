# Standalone guard configuration

`based serve` supports the same declared BSL guards as embedded Rust. Configure
all declared names before startup; incomplete or invalid mappings fail before
opening the listener or provisioning an idempotency table. The listener still
requires a trusted authentication edge that strips caller-supplied `X-Based-*`
headers and injects authorized context. Do not expose it directly to untrusted clients.

Create an operator-owned `guards.toml` (paths resolve from the project root):

```toml
[guards.caller_can_close]
endpoint = "https://policy.internal.example/guards/close"
secret_env = "BASED_CLOSE_GUARD_SECRET"
deadline_ms = 2000
```

Set the named secret in the process environment through your deployment's secret
manager, then run:

```sh
based serve --guard-config guards.toml
```

Secrets are never placed in CLI arguments, callback URLs, or the TOML file. Guard
secret variables are read from the process environment, not the project's `.env`.
Unknown guard names and unknown configuration fields are rejected. URLs reject
credentials and fragments. HTTPS verifies the certificate chain and hostname
using platform trust roots. For local development only, an explicit
`--guard-allow-loopback-http` permits HTTP on `127.0.0.1`, `[::1]`, or `localhost`.
It never permits HTTP on another host or disables HTTPS verification. Redirects,
automatic retries, ambient proxy selection and response decompression are disabled.
Restrict outbound access to your trusted callback services.

The callback authenticates the configured bearer token and implements the exact
[protocol v1 request and verdict](../spec/external-guards.md). The default total
deadline is two seconds; `deadline_ms` must be 1–30000. Requests are bounded to
256 KiB and responses to 16 KiB even for chunked bodies. Only an exact authenticated
200 JSON allow response proceeds. A deny returns `403 guard_denied` with its public
message; transport, deadline, authentication and protocol failures return the same
code with `Guard check unavailable`. Logs identify only the guard and failure class.

Checks run before argument validation, idempotency lookup/claim, and database
checkout. Replays recheck current permission; internal transaction retries do not
invoke the callback again. Cancelling the dispatch drops its outbound request and
never starts the write. This does not undo work already received by the callback;
keep callbacks read-only and safe to repeat. Axum does not promise that a socket
disconnect cancels a running handler, so bound callback work with the deadline.

Policy data reads require a separately authenticated restricted query path, with
server-derived tenant/user context. Approval is a preflight permission check;
atomic state invariants belong in database write conditions or transaction locks.

Embedded callers can opt into `based-runtime/external-guards`, register
`external_guard::HttpGuard::new(...).register(Guards::new())`, and pass the registry
to `Engine::with_guards` or `http::serve_with_guards`. Ordinary embedded closures
continue to work; embedded consumers do not require the HTTP adapter dependency.

Verification lives in `tests/external_guards/` (real SQLite effects, cancellation,
replay and transaction retry), the strict response/configuration unit tests, the
controlled TLS test, and `based-cli/tests/serve_guards.rs`. The TLS test generates
short-lived local certificates using OpenSSL; no public network service is needed.
