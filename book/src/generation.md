# Generated clients and artifacts

## The Rust client

Rust applications generate their client during `cargo build`. Add `based-build`
from the same pinned Git revision as your runtime:

```toml
[build-dependencies]
based-build = { git = "https://github.com/oscscull/baseddsl.git", rev = "..." }
```

Keep BSL in a schema directory and choose the client mode in `based.toml`:

```toml
root = "schema"
[generate]
client_mode = "embedded"
```

`build.rs`:

```rust
fn main() {
    based_build::run();
}
```

Include the client from `OUT_DIR`:

```rust
#[allow(dead_code, reason = "not every generated item is exercised")]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}
```

`based init --mode embedded` sets all of this up. The build reruns generation
when `based.toml` or any file under the schema directory changes, and skips it
otherwise. Invalid BSL fails the build with the source location. Nothing
generated lands in your source tree, so `cargo fmt` and Clippy configuration
never touch it, and rust-analyzer resolves the include after its first build.
The helper adds a one-time compile of the BSL compiler to cold builds.

The build also writes `OUT_DIR/schema.rs`, embedding `based.toml` and the BSL
for `Compiled::from_sources`, and `migrations/` as `schema::MIGRATIONS` (see
[a small Rust application](embedded.md)). When `migrations/` exists, a build
fails on anything `based migrate verify` rejects. Connection setup and applying
migrations stay explicit. `Compiled::load` reads
`based.toml` and BSL from disk instead.

The application needs `serde` with `derive`, `serde_json`, and its
runtime/transport dependencies. A wire client with decimal fields also needs
`bigdecimal = "0.4"`; an embedded client reaches it through `based_runtime`.
Entity IDs are distinct Rust types; an owner ID cannot stand in for an item ID.

## Checked-in artifacts

To commit generated output instead, or for SQL, OpenAPI, and other non-Rust
artifacts, configure owned destinations:

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
Include a checked-in client with `include!("../generated/client.rs")`.

Generation refuses handwritten outputs unless you explicitly use `--force`.
It also refuses symlinks and migration-history destinations. Identical files keep
their timestamps. Each replacement is atomic; a late filesystem failure may leave
some files updated, and the error identifies that partial set. Generation never
applies migrations.

## Inspecting SQL

`based explain` prints the statements a query or mutation executes, in run
order, exactly as the driver receives them: positional placeholders for the
project dialect, then what each one binds.

```sh
based explain . order_by_id
based explain . place_order --dialect postgres
based explain .            # every callable; same as --all
```

Binds read `$param`, `$param present (1/0)` for an optional filter, `$ctx.field`,
or `name (engine)` for values the engine supplies (minted ids, cursors). An
array param expands to one placeholder per element at run time.

Set `BASED_LOG_SQL=1` to log every executed statement to stderr from
`based serve` or an embedded `Engine`, with its bind count. Bound values are
never logged.
