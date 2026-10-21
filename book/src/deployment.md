# Deploying the service

## Network boundary

Put `based serve` behind an authenticated edge. The edge must discard caller
`X-Based-*` headers and inject authorized context and shard routing. Restrict
network access to the Based listener and guard callbacks so callers cannot bypass
the edge. Protect callback credentials separately from caller credentials.
The [standalone tutorial](standalone.md) builds a minimal edge.

## Running it

Deploy the matching CLI, `based.toml`, BSL, and reviewed migration history.
Apply migrations separately; starting the service does not apply application DDL.
Provide connection URLs through deployment secrets and use
[verified database TLS](tls.md) for remote servers.

```sh
based serve --listen 127.0.0.1:9090 --database-url local.db \
  --init-idempotency-table
```

The explicit setup flag creates only the idempotency table. Once provisioned,
start without it using a restricted runtime role. See [replay semantics](idempotency.md).

| Setting | `based serve` | Embedded listener (`based_runtime::http`) |
|---|---|---|
| Listen address | `--listen`, `BASED_LISTEN` | `ServeConfig::listen` |
| Databases and shards | `--database-url`, `BASED_DATABASE_URL` | Your backend |
| Pool size | `--pool-min`, `--pool-max` | Your pool |
| Context | Trusted `X-Based-Context` header | Any `ContextSource` |
| Idempotency store | `--idempotency-store` | Any store, `serve_with_store` |
| Guards | `--guard-config` callbacks | `Guards`: closures or callbacks |
| Shutdown | SIGTERM/SIGINT drain | `Handle::shutdown` |
| Statement log | `BASED_LOG_SQL=1` | `BASED_LOG_SQL=1` (also `Engine`) |

Request bodies are capped at 1 MiB in both; responses are not capped. Pool limits
bound database connections, not HTTP concurrency. Put request concurrency and
rate limits at the edge.

`/readyz` checks readiness; `/healthz` checks liveness. On shutdown the
listener stops accepting requests and drains active work. Allow enough time before
force-killing it; a forced kill can interrupt work. This does not guarantee a
fixed drain deadline or zero-downtime schema changes.

The container reads BSL from `/app`. Mount application assets read-only and
publish only the edge; a loopback diagnostics port is suitable for local checks.
The [Dockerfile](../../docker/Dockerfile) builds the service image.
