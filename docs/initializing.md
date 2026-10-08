# Start from an empty directory

Use the [versioned CLI installation](installation.md). `based init` is a small,
noninteractive initializer: choose an explicit mode; SQLite is the zero-server
default. It prepares and checks source, generated artifacts, and an initial
migration offline before creating the destination. It never applies a migration
or provisions a database server.

## Embedded walkthrough

Continue with the [embedded tutorial](embedded-tutorial.md) for the parent relation,
host context, error handling, and a data-preserving rename.

Prerequisites: `based`, Rust 1.94+, Cargo/Git, and platform C build tools. In an
empty directory, run:

```sh
based init --mode embedded
based migrate apply --database-url local.db
cargo run
```

Required decisions: **embedded mode**. The dialect defaults to SQLite; there is no
connection-secret, Docker, npm, framework, or generator choice. No code edits are
needed. The two commands after init apply the reviewed initial migration and run
the typed consumer. Output includes `created:` and `read:` with the same UUID and
`Hello Based` name.

The app demonstrates one optional parent relation and an owner scope, and uses
`generated/client.rs` through `include!`, a Git library revision
pinned to the CLI's source commit, and the production UUID generator. There is no
build script. The explicit-generation default follows the
[consumer build-cost decision](consumer-build-cost.md). `cargo fmt --check` formats
ordinary source without traversing the generated include; `cargo clippy -- -D
warnings` checks the consumer without application-wide lint suppression. The
allowance on the generated client module covers its intentionally unused API.

## Standalone walkthrough

Prerequisites: `based` and Python 3.9+ with its standard library. In another empty
directory:

```sh
based init --mode standalone
based migrate apply --database-url local.db
python3 demo.py
```

On Windows, use `py -3 demo.py` for the last command. Required decision:
**standalone mode**. No Rust or npm client build is needed. The demo starts the
installed `based serve` on an available loopback port, waits for readiness, posts
`/m/create_item` and `/q/items`, prints matching create/read results, and stops
its child service. Local replay storage is explicitly process-memory and disappears
on restart. Generated SQL and OpenAPI live under `generated/`.

This local demo supplies a fixed owner context directly; owner UUIDs are not credentials. Use the
[trusted-edge deployment guide](standalone-deployment.md) before exposing a
service. TypeScript authentication/guard integration is an optional later lesson.

## Change the schema explicitly

Add `description: text?` to `Item` in `schema/item.bsl`. Then:

```sh
based gen all
based migrate gen
based migrate verify
based migrate apply --database-url local.db
cargo run
# Standalone: use python3 demo.py (Windows: py -3 demo.py) instead of cargo run.
```

Review the new `migrations/` files before applying them. Generation and offline
migration planning do not change the database. Both demos run again without
consumer code edits. Add the field to `ItemView` if you also want it in the returned projection. Commit source, generated
artifacts, migrations, and an embedded app's `Cargo.lock`.

## Choose a server dialect

```sh
based init --mode embedded --dialect postgres
# Or: --mode standalone --dialect mariadb
```

Supply your own reachable database and export `DATABASE_URL` for the CLI and
consumer/demo. `.env.example` gives the URL form without real credentials.
Use `based migrate apply`, then `cargo run` or the Python demo. Nothing starts,
installs, or silently provisions a MariaDB/Postgres server. Embedded server drivers
include Rustls TLS support; configure server trust and permissions for deployment.

## Preserve existing work

`init` only accepts an empty destination, including when re-run. It refuses files,
symlinks, existing Rust projects, config, or data and explains how to use a new
sibling directory. It prepares the complete starter first and creates destination
files with non-overwriting writes. A filesystem failure can leave a partial new
starter; inspect it and choose an empty destination rather than replacing source.

The starter ignores `.env`, database files, SQLite sidecars, and build artifacts.
The CLI can read project `.env`; the Rust consumer and Python demo use exported
process configuration. SQLite requires no `.env` and defaults to `local.db`.
Never commit credentials or demo database contents.

The portable `make ci-initializer` gate reuses these exact starter fixtures. It
replays both starts and schema updates, checks no implicit migration application,
compiles both server-driver consumers, and verifies UUIDs, formatting, linting,
freshness, and collision preservation. Its isolated Git transport mirrors the
committed source for CI merge commits; generated manifests retain the public URL
and exact revision.
