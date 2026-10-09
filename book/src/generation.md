# Generated clients and artifacts

Configure owned destinations in `based.toml`:

```toml
[generate]
client = "generated/client.rs"
client_mode = "embedded"
sql = "generated/schema.sql"
openapi = "generated/openapi.json"
```

```sh
based gen all
based gen all --check
```

The second command checks freshness without writing. Individual generators still
support stdout and `-o` overrides. Paths resolve from the project manifest.

Generation refuses handwritten outputs unless you explicitly use `--force`.
It also refuses symlinks and migration-history destinations. Identical files keep
their timestamps. Each replacement is atomic; a late filesystem failure may leave
some files updated, and the error identifies that partial set.

Include generated Rust outside ordinary source modules:

```rust
#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}
```

Cargo formats the wrapper and leaves the included artifact alone. Review and
commit the generated output. Use the same compiler version and options when
regenerating. Generation neither applies migrations nor bundles runtime schemas.
Deploy `based.toml` and BSL alongside applications using `Compiled::load`.

The application needs `serde` with `derive`, `serde_json`, and its
runtime/transport dependencies. Decimal clients also need `bigdecimal = "0.4"`.
Entity IDs are distinct Rust types; an owner ID cannot stand in for an item ID.
