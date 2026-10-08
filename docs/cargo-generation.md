# Optional Cargo client generation prototype

`based-build` is an optional **build dependency**, retained as the
[#80](https://github.com/oscscull/baseddsl/issues/80) prototype. It is not a runtime
dependency or the starter default. The completed
[#54](https://github.com/oscscull/baseddsl/issues/54)
[build-cost comparison](consumer-build-cost.md) selects explicit
[generation into an isolated directory](generated-artifacts.md) because of the
measured edit-loop penalty and uncertainty in the initial cold series. Use this
helper only when deliberately choosing that tradeoff.

## Minimal integration

For a local checkout, add this build dependency alongside the ordinary generated
client dependencies (`serde` with `derive`, `serde_json`, and, for embedded mode,
`based-runtime` and `tokio`):

```toml
[build-dependencies]
based-build = { path = "/path/to/baseddsl/crates/based-build" }
```

At the Cargo package root, `based.toml` supplies the same compiler target and
client mode as the CLI:

```toml
dialect = "sqlite"
root = "schema"

[generate]
client_mode = "embedded"
```

Put BSL under `schema/`. A dedicated schema directory lets Cargo avoid running the
build script after unrelated application edits. The manifest and schema are
compiler inputs; the `build.rs` is only:

```rust
fn main() {
    based_build::generate().unwrap_or_else(|error| panic!("{error}"));
}
```

Use this small wrapper in application code:

```rust
#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}
```

`cargo build` creates the current client without an installed `based` binary,
`rustfmt`, manual generation, database connection, or migration. `cargo fmt`
formats the wrapper and leaves the generated output alone. Wire mode, the
manifest default, supplies the transport interface; embedded mode additionally
supplies the runtime transport adapters. Consumer dependencies and optional
driver-feature forwarding follow the existing
[generated consumer contract](generated-consumer-contracts.md).

## Input and output lifecycle

The helper watches `based.toml`, the schema directory recursively, and discovered
BSL files. This covers edits, new files/directories and deletions. It also watches
the generated client so deleting or changing it triggers cache validation. Generation
options live in the tracked manifest. Each actual build-script invocation first
reads the discovered inputs and compares their content fingerprint. Unrelated
Rust edits never invoke BSL parsing/checking/client emission. For layouts where
BSL and application files share a directory, Cargo may rerun the build script,
but the fingerprint check skips the compiler when its inputs are unchanged.

The fingerprint includes manifest bytes, sorted source paths and bytes, and the
host build-script executable bytes. Changing the helper/compiler during local
development invalidates the cache even if its package version stays the same.
A cache hit also checks the generated client bytes. Its discovery, file reads and
hashing are real build costs, included in the forthcoming comparison.

The fixed destination is `OUT_DIR/client.rs`; `[generate]` output paths are used
by explicit CLI generation, not by the helper. Checked inputs are rendered before
publication. The shared artifact writer preserves identical output mtimes,
refuses handwritten/symlink outputs, and atomically replaces a changed client.
The internal fingerprint record is also replaced atomically. A missing/corrupt
record is a cache miss. No cached client can satisfy a build after a bad input
edit: checking fails with the diagnostic code and source path, line and column,
and the build stops. A previous client may remain on disk after failure; Cargo
cannot use it to complete that failed build.

`generate()` returns a `Generation` containing `path`, `checked` (whether this
invocation ran the compiler), and `changed` (whether it published changed bytes).
These fields allow verification and measurement without adding routine build
messages. They are not a stable compatibility promise while this remains a
prototype.

## Runtime assets and explicit generation

Client generation does not produce an engine, precompile runtime schemas, bundle
BSL, or provision a database. An embedded application still loads or supplies its
runtime schema, configures its backend and production ID generator, and manages
migrations explicitly. If it calls `Compiled::load`, deploy its manifest and BSL
assets and resolve their directory deliberately. The
[verification fixture](../ci/fixtures/cargo-generation/src/main.rs) uses a mock
backend to prove a typed call without a live database; applications use their
chosen real backend.

Explicit committed artifacts use **the same** `based-project` compiler and
client-option adapter, with the CLI's separate publication workflow:

```toml
[generate]
client = "generated/client.rs"
client_mode = "embedded"
```

```sh
based gen all
based gen all --check
```

The explicit wrapper uses `include!("../generated/client.rs")` inside the same
`#[allow(dead_code)] mod client` as above. Follow the
[artifact lifecycle](generated-artifacts.md) for ownership and regeneration.
Standalone and explicit consumers need no build dependency. Migrations retain
their reviewed generation/apply lifecycle in both approaches.

## Verification and cost decision

Run `make ci-cargo-generation`. It copies a fresh consumer to a scratch directory,
builds and calls its client with generation tools blocked on PATH, checks repeated
and unrelated-Rust builds, exercises schema edits/additions/deletions and mode
changes, verifies invalid input failure and formatter isolation, and checks that
`based-build` has no runtime, SQLx, async runtime, or HTTP dependencies. The job is
also included in CI. `make ci-workspace` checks the shared CLI compiler adapter.

A clean build compiles host-side compiler dependencies in addition to application
runtime dependencies; Cargo may compile overlapping crates twice for different
roles/features. Neither this cost nor generator/cache work is treated as free.
The [matched consumer benchmark and decision](consumer-build-cost.md) record
these costs and select explicit generation for the starter. No timing threshold is imposed in CI.
