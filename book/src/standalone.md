# A standalone HTTP service

`based serve` runs your schema as an HTTP service. Callers never reach it
directly: an authentication edge in front of it decides who they are.

You need the released `based` binary ([installation](installation.md)), Node.js
20+ for the example edge, and curl. No Rust, Cargo, or npm packages.

## Create and migrate

```sh
based init items --mode standalone
cd items
based migrate apply --database-url local.db
```

The starter has one scoped `Item` model with an optional parent, two callables
(`create_item`, `items`), and the edge in `edge/`. `generated/openapi.json`
describes each route's input and output.

## Start the service and the edge

In one terminal:

```sh
based serve --listen 127.0.0.1:9090 --database-url local.db --init-idempotency-table
```

`--init-idempotency-table` creates the table that stores retried results. You need
it once per database.

In a second terminal:

```sh
node edge/edge.mjs
```

The edge listens on port 8080. For each request it:

1. maps the bearer token to a context (`edge/identity.mjs`);
2. forwards only `POST /q/<query>` and `POST /m/<mutation>`;
3. builds fresh headers, so a caller's `X-Based-*` headers never reach Based.

Based trusts `X-Based-Context` without checking it. That is why only the edge may
reach port 9090. Both listen on loopback here; in production use a private
network or firewall ([deployment](deployment.md)).

## Call it

```sh
curl -s localhost:8080/m/create_item -H 'Authorization: Bearer alice-local-token' \
  -H 'Idempotency-Key: first-item' -d '{"name":"First","parent":null}'
curl -s localhost:8080/q/items -H 'Authorization: Bearer alice-local-token' -d '{}'
```

The scope fills `owner` from Alice's context. Bob cannot see her rows, even when
he forges her context, because the edge replaces the header:

```sh
curl -s localhost:8080/q/items -H 'Authorization: Bearer bob-local-token' \
  -H 'X-Based-Context: {"owner":"00000000-0000-4000-8000-00000000000a"}' -d '{}'
```

That returns `[]`. Without a token the edge answers `401`:

```sh
curl -s localhost:8080/q/items -d '{}'
```

## Retry safely

Run the first `create_item` command again. It returns the same item; nothing new
is written. Now stop `based serve` (Ctrl-C) and start it without the setup flag:

```sh
based serve --listen 127.0.0.1:9090 --database-url local.db
```

The same request still returns the original item: the key and its result were
committed with the write. Reusing the key with different input is rejected with
`422 idempotency_key_reuse`:

```sh
curl -s localhost:8080/m/create_item -H 'Authorization: Bearer alice-local-token' \
  -H 'Idempotency-Key: first-item' -d '{"name":"Changed","parent":null}'
```

See [retrying mutations](idempotency.md) for retention and store choices.

## Change the schema

Stop `based serve`. In `schema/item.bsl`, add `note: text?` to `Item` and `note`
to `ItemView`, then:

```sh
based gen all
based migrate gen
based migrate verify
based migrate apply --database-url local.db
```

Start `based serve` again. Existing rows return `"note": null`. Generation never
touches the database; review each migration before applying it
([migrations](migrations.md)).

## Next

- Replace `edge/identity.mjs` with your identity provider. Keep step 3.
- Run named permission checks as HTTP callbacks: [guards](guards.md). The
  [TypeScript guard example](https://github.com/oscscull/baseddsl/tree/main/examples/standalone-typescript-guards)
  shows an edge, a callback, and denied writes end to end. Callbacks decide
  permission; put state invariants in write conditions.
- Before exposing anything, read [deployment](deployment.md).
