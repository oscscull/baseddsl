# Test fixtures

Fixtures seed test rows from typed builders, one per model, so a test never writes raw
`INSERT`s.

A builder fills only what the row needs:

- A required column with no default gets a placeholder: `"<field>-<n>"` for text, `n`
  for numbers, `false`, the first enum variant, 2000-01-01 plus `n` seconds or days, `{}`,
  or empty bytes. `n` comes from a per-process sequence that starts at a random offset,
  so unique columns never collide, even when parallel test processes share a database.
- A required relation is a constructor argument. Fixtures never create a parent for you.
- Every other column stays unset unless you set it, so it is `NULL` or its database
  default. Ids are minted as usual, and `@created`/`@updated` still stamp.

Inserts bypass scopes and guards, and `insert` returns the whole stored row. Fixtures
are never exposed as HTTP routes. On a sharded deployment, `.on_shard(key)` picks the
shard by its routing key; the default is the empty key.

## Rust

Every generated Rust client includes a `fixtures` module, whether `build.rs` or
`based gen client` produced it. An embedded client inserts through any engine-backed
client: `client::embedded`, the client inside `client::transaction`, or an `adopt_*`
client.

```rust
use client::fixtures;

let api = client::embedded(&engine);
let acme = fixtures::org().name("Acme").insert(&api).await?;
let buyer = fixtures::user().insert(&api).await?;
let order = fixtures::order(&acme, buyer.id.clone())
    .status("paid")
    .insert(&api)
    .await?;
```

A wire client, or any test without an engine, inserts through a `based fixtures`
process instead. It reads requests on stdin and opens no socket; dropping the connection
ends it.

```rust
let db = fixtures::connect(fixtures::Connect::new("test.db").cwd("path/to/project"))?;
let acme = fixtures::org().insert(&db).await?;
```

A relation argument takes a row or its id. A preset is an ordinary function:

```rust
fn paid_order(org: &fixtures::OrgRow, buyer: &fixtures::UserRow) -> fixtures::OrderFixture {
    fixtures::order(org, buyer).status("paid")
}
```

## TypeScript

Generate the module, then connect once per test suite. `connect` starts the same
`based fixtures` process for the project in `cwd`.

```sh
based gen fixtures --lang ts -o src/generated/fixtures.ts
```

```ts
import * as fixtures from "./generated/fixtures.js";

const db = await fixtures.connect({ databaseUrl: "test.db" });
const acme = await fixtures.org().name("Acme").insert(db);
const buyer = await fixtures.user().insert(db);
const order = await fixtures.order(acme, buyer).status("paid").insert(db);
await db.close();
```

A model whose required column has an opaque `raw(…)` type has no builder.
