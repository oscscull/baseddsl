# Deploying `based serve` as a container

The image compiles the `based` binary and runs `based serve` — the HTTP listener over the
dispatch core, with `/healthz` (liveness) + `/readyz` (readiness) probes and graceful drain
on `SIGTERM`. It serves whichever dialect the project's `based.toml` targets
(MariaDB, Postgres, or SQLite).

The image carries **no schema** — you supply your project (`based.toml` + `**/*.bsl`
[+ `migrations/`]) at `/app`, and configure everything else by env. One image, any project.

## Build

```sh
docker build -f docker/Dockerfile -t based-serve .
```

Multi-stage: a Rust builder compiles the release binary (BuildKit cache mounts keep an
unchanged dependency set from recompiling); a `debian:bookworm-slim` runtime carries just
the binary + entrypoint (~120 MB). Runs as an unprivileged user.

## Configuration (all env)

| env | meaning |
|-----|---------|
| `BASED_DATABASE_URL` | one shard URL, or comma-separated for a sharded fleet. **Required.** The standard `DATABASE_URL` is also honored. |
| `BASED_PROJECT` | served schema root — mount your project here. Default `/app`. |
| `BASED_LISTEN` | bind address, read natively by `based serve`. Default `0.0.0.0:8080`. |
| `BASED_MIGRATE_ON_START` | `1` runs `based migrate apply` before serving. Off by default — leave unset if you apply migrations out of band. |

`$ctx` (auth/scope) is **server-supplied, never the request body**: front the
container with an auth proxy that sets `X-Based-Context` (a JSON object) after
authenticating the caller. See `spec/syntax/auth.md`.

## Run (Postgres example)

```sh
docker run -d --name based-serve -p 8080:8080 \
  -v "$PWD/examples/postgres-quickstart:/app:ro" \
  -e DATABASE_URL="postgres://user:pw@db-host:5432/mydb" \
  -e BASED_MIGRATE_ON_START=1 \
  based-serve
```

Then:

```sh
curl -s http://127.0.0.1:8080/healthz            # 200 while serving (liveness)
curl -s http://127.0.0.1:8080/readyz             # 200 when the DB is reachable (readiness)

# A public mutation (no scope) — seed a tenant, keep its id:
curl -s -X POST http://127.0.0.1:8080/m/create_org \
  -H 'content-type: application/json' -d '{"name":"Acme","slug":"acme"}'

# A scoped read — $ctx names the tenant (set by your auth proxy in real deploys):
curl -s -X POST http://127.0.0.1:8080/q/my_orders \
  -H 'content-type: application/json' \
  -H 'X-Based-Context: {"org":"<org-id>"}' -d '{}'
```

The wire is `POST /q/<name>` (queries) and `POST /m/<name>` (mutations), body = the
argument object (`calling.md`).

## SQLite

SQLite is bundled (no service). Point `DATABASE_URL` at a file **on a writable volume**
(the mounted project is read-only, so the DB file cannot live under `/app`):

```sh
docker run -d -p 8080:8080 \
  -v "$PWD/examples/sqlite-quickstart:/app:ro" \
  -v based-data:/data \
  -e DATABASE_URL=/data/app.db -e BASED_MIGRATE_ON_START=1 \
  based-serve
```

## Health & shutdown

- `HEALTHCHECK` probes `/healthz` — never touches the DB, so a DB blip drains via `/readyz`
  rather than restarting an otherwise-healthy box.
- On `SIGTERM`/`SIGINT` the server flips `/readyz` to `503` first (a load balancer pulls the
  instance out of rotation), lets in-flight requests finish, then exits — zero-downtime
  rolling deploys. `docker stop` (SIGTERM) drains cleanly.

## Overriding the command

A non-flag argument runs verbatim, so the image doubles as a CLI:

```sh
docker run --rm -v "$PWD/examples/postgres-quickstart:/app:ro" \
  -e DATABASE_URL="postgres://…" based-serve based migrate status
```

CI builds + smoke-boots this image on every push (`make ci-image`).
