# Generating during a Cargo build

Explicit generation is the starter default. To generate the client during a
Cargo build, add `based-build` from the same pinned Git revision as your runtime
under `[build-dependencies]`. Keep BSL in a dedicated schema directory:

```toml
root = "schema"
[generate]
client_mode = "embedded"
```

In `build.rs`:

```rust
fn main() {
    based_build::generate().unwrap_or_else(|error| panic!("{error}"));
}
```

In your application:

```rust
#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}
```

The helper tracks manifest/schema edits, additions, and deletions. Its content
cache skips compiler work after unrelated Rust edits; deleting or modifying the
output triggers regeneration. Invalid BSL stops the build with source diagnostics.
The destination is always `OUT_DIR/client.rs`; configured explicit output paths
are ignored by this helper.

This adds compiler dependencies to the host build and cache-check work to the edit
loop. Choose it deliberately. Runtime schemas, connection setup, and migrations
remain your application's responsibility.
