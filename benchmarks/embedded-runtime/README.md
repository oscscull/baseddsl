# Embedded runtime / direct SQLx benchmark

This bounded SQLite experiment measures the generated embedded Rust client's
overhead over direct SQLx using the **same executed SQL and final Rust types**.
It is a diagnostic workload, not a general database leaderboard.

From the repository root:

```sh
make ci-runtime-benchmark
cargo build --locked --release --manifest-path benchmarks/embedded-runtime/Cargo.toml
mkdir -p benchmarks/embedded-runtime/results
benchmarks/embedded-runtime/target/release/embedded-runtime-benchmark 200 \
  > benchmarks/embedded-runtime/results/run-1.json
# Repeat the executable in fresh processes at least three times.
# macOS: /usr/bin/time -l <executable> 200 records whole-process peak RSS.
# Linux: /usr/bin/time -v <executable> 200 records whole-process peak RSS.
```

Samples default to 200 and must be 5–10000. CI uses five samples to exercise
correctness; it has no latency threshold. Compile before measuring and record
the toolchain, CPU, OS, power mode, competing load, revision, and dependency lock.
The machine-readable JSON includes median, p95, minimum, maximum, measured
operations/second, input/result serialized bytes, actual SQL/binds, and query plans.
Serialized byte lengths describe payload sizes, **not heap allocations**.

## Matched workload

Both paths use the same in-memory database, bundled SQLite build, single SQLx
connection pool, schema, and seed: 128 owners with eight items each (1024 total).
Flat reads return 8 or 1024 rows; nested reads return 1 or 128 owners plus children;
the offset page returns 32 rows plus the total; bulk inserts write 8 or 512 items.
The large bulk crosses the runtime's 900-bind budget: two INSERTs in one
transaction. Both paths construct the typed bulk input and SQL/binds inside
their timed operation, commit, and return an acknowledgement without read-back.

Before timing each workload, a separate recording backend captures its executed
statements. Direct SQLx executes that SQL with the same bindings and native row
decoding into the generated result types. Nested reads use the same SQL-side JSON
aggregation and decode the same child-array JSON. Pagination executes its main
SELECT and COUNT on one checked-out connection in both paths. Write verification
compares all newly persisted rows, including serial IDs, after resetting between
paths. Known fixture cardinalities, flat values, page offset/total, and SQL counts
are checked too. The timed engine has no recording wrapper.

Each path warms ten operations and then records individual awaited operations.
The measured interval ends after full result materialization, before dropping
the result. Write cleanup restores data and the serial sequence outside every
interval. Paths run sequentially, with their order alternating between workloads;
pool/statement caches are warm. Throughput is the reciprocal of measured serial
operation latency, excluding warmup, reset, and reporting; it is not concurrent
server throughput. SQL counts exclude pool checkout, BEGIN/COMMIT, and cleanup.

## What the measurements distinguish

The warm embedded path includes input/context serialization, dispatch,
validation/planning/binding, driver row-to-JSON conversion, response shaping, and
generated result deserialization. The direct path uses precompiled captured read
SQL, matching bindings and typed SQLx row decoding. Their difference includes
all those engine responsibilities; it does not isolate a single implementation
step. Both pay SQL execution and final result materialization.

CPU-only probes separately measure typed input → JSON value and an already
materialized JSON value → typed output. Fixture construction and JSON-value
cloning are outside those probe intervals. They exclude database work,
row-to-JSON conversion, nested JSON parsing in the engine, dispatch, and planning.
They are diagnostic estimates, not an additive decomposition of end-to-end time.
No text JSON/network round-trip occurs in the embedded bridge.

`startup_schema_load_us` times the first `Compiled::load` in the process: manifest
discovery, parsing, checking, and SQL lowering. It excludes database initialization,
seeding, and Rust compilation. The first process load can still benefit from OS
filesystem caches. Consumer compilation costs have a separate [report](../../docs/compile-time.md).

## Limits

SQLite has no network latency, remote server contention, or realistic shard
distribution here. There is one connection and one caller. The seed is small,
the text fields are short, and query plans are SQLite-specific. No decimal,
streaming, guard callback, keyset cursor, idempotency store, or HTTP listener is
measured. Serial keys avoid production ID-generation work. Cross-driver or
production claims require their own matched measurements.

Peak RSS from the external time command covers the **whole process**, including
both paths, fixture, cached statements, report, runtime and allocator; it cannot
attribute memory to either path. This harness does not count allocations or
per-operation live bytes. Do not interpret serialized payload bytes as memory.
Machine load, scheduler wakeups and path order can dominate small absolute
differences. Publish repeated runs and variance alongside ratios; do not infer
a performance win or a universal overhead percentage from this workload.

See [measured results](../../docs/runtime-performance.md).
