# Generated artifact lifecycle

Configure repeatable generation in the project's `based.toml`:

```toml
[generate]
client = "generated/client.rs"
client_mode = "embedded" # "wire" is the default
sql = "generated/schema.sql"
openapi = "generated/openapi.json"
```

Destinations are resolved from the manifest directory, including when a command
starts in a nested schema/application directory. Explicit absolute output paths
are also supported. Parent directories are created only when writing. Migration
history under the project's `migrations/` remains protected from these commands,
including `--force` and directory aliases. Use `based migrate gen` for its
separate reviewed, append-only lifecycle.

```sh
based gen all          # every configured destination, one checked schema
based gen all --check  # CI freshness gate: no files/directories created or modified
based gen client       # just the configured client
based gen sql          # just the configured SQL
based gen openapi      # just the configured contract
```

Missing or stale outputs make `--check` exit nonzero. A current output retains its
bytes and mtime during either checking or ordinary regeneration. A schema
edit/addition/deletion can make the configured client stale; regeneration must
succeed before the new client is published. Parse/type errors include source
locations and leave existing artifacts untouched.

## CLI overrides

```sh
based gen client --out scratch/client.rs --mode wire
based gen client --out scratch/client.rs --mode embedded
based gen client --embedded # compatibility alias for --mode embedded
based gen client --out scratch/client.rs --mode embedded --check
```

`--out` overrides only that artifact's configured path. `--mode` overrides
`generate.client_mode`; `--embedded` also overrides it, and conflicts with
`--mode`. Without a configured destination or `--out`, individual commands keep
their stdout behavior. `--check`/`--force` require a destination. `gen all`
requires at least one configured destination and uses the configured paths/mode.
The optional positional root still overrides ancestor manifest discovery.

## Ownership and replacement

Rust and SQL headers identify generated ownership and give the exact generation
command to run **from the manifest directory**. OpenAPI carries equivalent
`x-based-generated.owner` and `x-based-generated.regenerate` metadata. Header
commands use POSIX shell quoting when paths need quoting. The writer recognizes
existing Based Rust/SQL generation headers, including earlier versions.
OpenAPI files without the ownership extension need a one-time explicit override
when adopting them into this lifecycle.

An existing handwritten file is refused before any artifact in the batch is
replaced. Review the file before deliberately replacing it:

```sh
based gen client --out generated/client.rs --force
# Or adopt all reviewed configured destinations:
based gen all --force
```

`--force` is not a migration-history override. File symlinks and non-file
destinations are refused. Generated ownership is a convention, not a signature:
editing a file while keeping its generation marker leaves it generator-owned,
and future generation may replace those edits.

All generation finishes before publication. The shared `based-artifacts` writer
preflights ownership/destinations for the complete set, then writes and syncs
temporary siblings for every changed output before replacing any target. Each
replacement uses an atomic rename and preserves existing file permissions.
Staging failures remove the temporary files and leave artifact contents intact
(new parent directories can remain). A multi-file publication cannot be globally
atomic: a late filesystem failure reports exactly which paths were replaced and
that remaining outputs were not replaced. Repair the filesystem and rerun the
same command; it never reports a successful complete set on that failure. Use
one generator per project at a time; this is not a cross-process filesystem lock.

## Rust consumer wiring

Keep generated declarations outside application `src/` and include them through
a small wrapper; the emitter owns their formatting:

```rust
#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}
```

Application `cargo fmt` formats the wrapper without walking the included file.
The wrapper's dead-code allowance is scoped to generated declarations because
each application uses only part of the generated API.

Every Rust client requires `serde = { version = "1", features = ["derive"] }`
and `serde_json = "1"`. Decimal fields additionally require `bigdecimal = "0.4"`.
Embedded mode requires `based-runtime` and the backend features the application
uses, such as `sqlite`, `postgres`, or `mariadb`; production UUID/ULID keys require
`id-gen`. Server-driver certificate verification uses `tls-rustls`. A wire-mode
client does not require `based-runtime`; supply its `Transport` implementation.
See the [fresh consumer contracts](generated-consumer-contracts.md).

The optional `adopt_sqlite`/`adopt_postgres`/`adopt_mariadb` transaction helper
retains its consumer driver gate. If using that helper, forward the matching
feature, for example `postgres = ["based-runtime/postgres"]`. If the alias is
absent, the helper is intentionally omitted and its scoped `unexpected_cfgs`
allowance avoids noise. Ordinary embedded calls need no consumer feature alias.

The examples configure explicit `generated/` artifacts and one regeneration
command. The [measured build-cost gate](consumer-build-cost.md) selects explicit
generation for the starter; optional Cargo `OUT_DIR` generation remains a prototype.
Client generation does not load runtime schema assets into the executable or run
database migrations. Continue to ship/load the schema and run migrations explicitly.

The optional [Cargo generation prototype](cargo-generation.md) shares the compiler
and emitter and writes only to `OUT_DIR`; the starter uses the explicit workflow.
