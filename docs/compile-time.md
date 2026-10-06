# Compilation costs

Measured on 2026-10-05 against main `d622856`, using stable Rust 1.97.0 on an
Apple M4 Max (14 CPU cores, 36 GiB RAM), with 14 Cargo jobs. Samples were captured
at `84f57b7`, before the async-trait 0.1.92 compatibility update for Rust 1.99
Clippy. These are local
measurements, not timings from a GitHub Actions runner. Headstart was not used.

The priorities are downstream application development and cold CI compilation.
The standalone examples use their own Cargo profiles: workspace profile changes
do **not** propagate to consumers.

## Results

Each entry is the median of three runs per revision, in alternating before/after
order. Cold means a fresh target directory with registry sources already cached;
network downloads are excluded. Incremental means a one-line function-body edit
in the application's `main`, or in `based_parser::parse_file` for workspace tests.
No-op builds are not used as incremental measurements.

| Compilation | Before | After | Time saved |
|---|---:|---:|---:|
| SQLite app, cold dev | 8.15s | 8.13s | 0.2% |
| SQLite app, dev body edit | 0.45s | 0.46s | -2.1% |
| Postgres app, cold dev | 9.31s | 9.06s | 2.7% |
| Postgres app, dev body edit | 0.51s | 0.48s | 6.2% |
| Postgres app, cold release | 12.88s | 12.50s | 2.9% |
| Helpdesk app, cold dev | 12.46s | 12.02s | 3.6% |
| Helpdesk app, dev body edit | 0.62s | 0.60s | 3.0% |
| Workspace fast tests, cold compilation | 23.48s | 10.25s | 56.4% |
| Workspace fast tests, parser body edit | 2.27s | 2.21s | 2.8% |

The raw samples are in [compile-time.csv](compile-time.csv). Differences of a few
percent on subsecond rebuilds are noise-sized; these runs do not establish a
meaningful general downstream speedup. The large repeatable improvement is the
workspace's cold test compilation. Full test execution, Docker startup, extension
packaging, and network downloads are not included in these compilation timings.

## What the profile found

Cargo's `--timings` showed the old `profile.dev.build-override.opt-level = 3`
putting optimized proc-macro compilation on the critical path. `serde_derive`,
`async-trait`, `clap_derive`, and several ICU derives each occupied 8–9 seconds
(they overlap; these durations must not be added as wall time). An isolated
experiment removing only that override reduced cold test compilation from
25.45s to 13.00s. Cargo's default unoptimized build dependencies preserve
application optimization settings and did not slow the measured warm rebuild.
Release optimization and runtime behavior are unchanged.

The old fast gate also compiled both server driver stacks: the CLI enabled them
unconditionally, and runtime dev-dependencies independently enabled every sqlx
backend. The fast gate now disables the CLI's default server-driver features and
uses only SQLite/HTTP. Production CLI defaults still include MariaDB and Postgres;
`make check` still compiles and tests every driver. The codec spike enables its
extra decimal codec through `docker-tests`. `ci/check-fast-features.sh` guards the
fast dependency graph, including dev-dependencies.

Downstream consumers now avoid compiling the Rust-client and OpenAPI emitters
through the runtime's codegen dependency. Those remain default features of
`based-codegen`, so the CLI, editor, and direct codegen users retain both emitters.
Computed result typing is gated with the emitters; SQL numeric casts remain in
the runtime lowering path.

Embedded applications can request `based-runtime`'s `id-gen` feature for production
UUID/ULID generation without its HTTP listener. `serve` still includes `id-gen`,
so existing server builds are compatible. The MariaDB/Postgres quickstarts and
helpdesk select `id-gen`; the Postgres quickstart drops from 227 to 206 compilation
units. The helpdesk also disables unused Redis ACL, stream, geospatial, Lua, and
bigint features, retaining its async connection manager and basic key commands.

### SQLite production-ID dependency boundary

As of 2026-10-06 (main `a8871be` plus the #82 starter wiring), the SQLite
quickstart also selects `sqlite,id-gen` and uses `based_runtime::id::UuidGen`.
Counting distinct packages in `cargo tree -p based-runtime --no-default-features
--features sqlite,id-gen -e normal,build --prefix none --format '{p}'` gives 140;
selecting `sqlite,serve` instead gives 160. This removes 20 HTTP-related packages,
including axum, hyper and tower, while preserving production UUID/ULID generation.
The existing CI feature-boundary check now rejects HTTP dependencies in the
embedded graph. This is a dependency count, not a new timing measurement; the
SQLite timing samples above used the earlier test-ID starter and do not measure
the cost of production ID generation.

## Reproducing the comparison

Use separate copies of the baseline and candidate; do not clean the developer's
existing target directory. Prefetch dependencies before timing. Each cold run
gets a fresh target directory, and each incremental run reuses the corresponding
revision's warmed directory. Use the same compiler, job count, features, and
profile on both sides, except the intentional fast-gate feature correction.

```sh
# Standalone downstream consumer (its own default dev profile).
cargo build --manifest-path examples/postgres-quickstart/Cargo.toml \
  --locked --offline -j 14 --timings --target-dir /tmp/unique-cold-target

# Repeat with --release for production builds; use the sqlite-quickstart and
# axum-helpdesk manifests for the other downstream cases.

# Candidate's infra-free test compilation; baseline omits --no-default-features.
cargo test --workspace --no-default-features --features sqlite,serve --no-run \
  --locked --offline -j 14 --timings --target-dir /tmp/another-unique-target
```

Cargo writes its unit timeline to `<target-dir>/cargo-timings/cargo-timing.html`.
For the incremental sample, insert/change `let _ = std::hint::black_box(1usize);`
at the start of the function body in a disposable copy, then time the same
command against its warm target directory. Alternate revisions and use medians;
a single run was misleading for the downstream cases here.

Verification: `make ci-workspace`, the full `make check` gate (including live
MariaDB/Postgres/SQLite and all examples), and clippy for the minimal embedded
`sqlite,id-gen` runtime. The final Redis feature selection is additionally checked
by the live helpdesk smoke against Postgres and Redis.
