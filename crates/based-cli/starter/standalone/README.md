# Based HTTP service

Requires the matching `based` binary. The example edge needs Node.js 20+ and no
packages. SQLite uses local.db. Other dialects need DATABASE_URL; see .env.example.

```sh
MIGRATE
SERVE
```

In a second terminal, start the edge and call through it:

```sh
node edge/edge.mjs
curl -s localhost:8080/m/create_item -H 'Authorization: Bearer alice-local-token' -H 'Idempotency-Key: first-item' -d '{"name":"First","parent":null}'
```

Callers reach only the edge. It authenticates the bearer token, drops caller
`X-Based-*` headers, and injects the owner context. Replace `edge/identity.mjs`
with your identity provider. `generated/openapi.json` describes every route.

After editing schema/item.bsl, stop `based serve`, then:

```sh
based gen all
based migrate gen
based migrate verify
MIGRATE
SERVE
```

Review migrations before applying them. Generation does not alter the database.
Commit generated artifacts and migration history. Keep credentials and database
files out of source control.

Read [the standalone tutorial](https://github.com/oscscull/baseddsl/blob/main/book/src/standalone.md)
and [deployment](https://github.com/oscscull/baseddsl/blob/main/book/src/deployment.md)
before exposing the service.
