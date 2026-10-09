# Calling the HTTP service

The standalone initializer includes a small Rust HTTP client. It runs a local
Based process, creates and reads related items, then stops the process:

```sh
based init --mode standalone
based migrate apply --database-url local.db
cargo run
```

The example requires Cargo and uses a fixed local owner context. It demonstrates
the wire API; it is not an authentication service. Your actual caller can use any
HTTP client and needs no Rust dependency.

To run the service yourself:

```sh
based serve --listen 127.0.0.1:9090 --database-url local.db \
  --init-idempotency-table
```

Each callable accepts POST requests at `/q/<query>` or `/m/<mutation>`.
The body is its JSON input. An authenticated edge supplies `X-Based-Context`;
never accept that header directly from an untrusted caller.

For an authenticated edge and external permission callback, run the
[TypeScript guard example](guards.md#example). It demonstrates allowed and denied
writes, forged-header replacement, callback failures, and keyed retries.
See [deployment](deployment.md) before exposing a listener and
[idempotency](idempotency.md) for retries across process restarts.
