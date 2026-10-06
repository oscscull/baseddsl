# Generator-owned Rust clients

`based gen client` emits readable Rust directly from versioned templates and
emitters. The same schema, options and generator version produce the same bytes:
generation invokes no `rustfmt`, reads no consumer formatter configuration, and
has no tool-dependent fallback. `client_with` returns ready-to-write source;
`format_rust` remains a pure identity helper for existing API callers.

Keep the output outside the application's ordinary source modules:

```sh
mkdir -p generated
based gen client -o generated/client.rs --embedded
```

In your user-authored `src/main.rs` or `src/lib.rs`:

```rust
#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}
```

Use `pub mod client` when the application needs to export it. The small module
wrapper gives generated declarations their own namespace and limits the unused
item allowance to the generated client. `cargo fmt` formats that wrapper and
leaves the included artifact untouched, including with a custom `max_width`.
Do not declare `mod client;` pointing at the generated file, format it directly,
or use a whole-application formatter/lint exemption. Review and commit explicit
generated artifacts; regenerate with the same flags after editing BSL. Generation
never applies migrations. The generated header identifies ownership.

To migrate existing `src/client.rs` consumers, create `generated/`, regenerate
there with your existing options, replace `mod client;` with the wrapper above,
and remove the old generated `src/client.rs`. Existing call sites remain the
same. See the [SQLite quickstart](../examples/sqlite-quickstart/README.md).

Cargo `OUT_DIR` integration is a separate optional prototype in #80. It may
become the starter default only after the measured build-cost gate in #54 is
accepted; until then explicit generation plus `include!` is the supported path.
