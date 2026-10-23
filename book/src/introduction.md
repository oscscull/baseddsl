# Based

Based defines database models, queries, and mutations in `.bsl` files. The engine
executes them through an embedded Rust client, generated during `cargo build`,
or an HTTP service. Your application supplies authenticated context,
connections, and permission checks.

Start with [installation](installation.md), then [a small Rust application](embedded.md).
For HTTP callers, use [the standalone service](standalone.md).

Evaluate Based against your own requirements before adopting it.
