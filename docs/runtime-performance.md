# Embedded runtime measurements

This SQLite workload shows measurable embedded overhead for larger results and bulk writes. It does **not** support a general claim that the engine is faster than direct SQLx or has negligible overhead. Small-operation timings vary enough that their ordering is unreliable.

The [bounded harness](../benchmarks/embedded-runtime/README.md) compares the generated embedded client and direct SQLx with identical schema, data, SQL, pool, transaction boundaries, and final Rust result types. All seven workload parity checks passed in each process. See the [raw measurements, SQL/binds, and query plans](../benchmarks/embedded-runtime/measurements/apple-m4-max.json).

## Environment and method

Measured 2026-10-08 on an Apple M4 Max, macOS 27.0.1 (26A434), aarch64, Rust 1.99.0 (`b940084d7`), SQLx 0.9.0, bundled SQLite 3.51.3. The standalone consumer lock is recorded by SHA-256 in the raw report. Both paths share one in-memory SQLite pool with one connection. Release mode uses thin LTO. Each path warms ten operations and records 200 serial operations per workload in each of three fresh processes. Compilation finished before sampling. Desktop background load and power state were not controlled.

Reported latency bands below are the minimum–maximum of the three **run medians**, not confidence intervals or individual-operation ranges. Added latency is the difference of the paths’ median-of-run-medians. Individual-run p95/min/max and measured serial throughput are retained in JSON. Path order alternates between workloads; it is fixed across these three runs. Scheduler and order effects remain possible.

## Warm latency

| Workload | Direct SQLx median range (µs) | Embedded median range (µs) | Added median (µs) | SQL statements |
|---|---:|---:|---:|---:|
| Flat(8) | 67.6–88.7 | 86.0–114.0 | +13.1 | 1 |
| Flat(1024) | 1610.1–1800.9 | 3228.7–3473.4 | +1493.9 | 1 |
| Nested(1) | 75.4–112.5 | 82.1–111.8 | -12.3 | 1 |
| Nested(128) | 1586.3–1848.8 | 2423.7–2526.5 | +715.9 | 1 |
| Page | 121.0–177.0 | 155.6–212.6 | +36.4 | 2 |
| Bulk(8) | 86.1–108.9 | 112.2–140.4 | +32.6 | 1 |
| Bulk(512) | 616.4–808.0 | 1819.4–1982.5 | +1084.8 | 2 |

The median-of-run-medians ratios for the larger flat read, nested read, and bulk write are respectively 1.85×, 1.40×, 2.40×. Nested(1) reverses ordering across runs: its aggregate negative difference is **not evidence of a performance advantage**. SQL counts exclude BEGIN/COMMIT, checkout, and fixture cleanup. The 512-row bulk executes two INSERTs under the same transaction; pagination executes a SELECT and COUNT. Nested reads use one correlated JSON-aggregation SELECT in both paths, not additional client-side child queries.

## Serial throughput

| Workload | Direct SQLx ops/s range | Embedded ops/s range |
|---|---:|---:|
| Flat(8) | 11959.5–12994.9 | 9142.5–10451.0 |
| Flat(1024) | 518.4–553.6 | 272.8–295.4 |
| Nested(1) | 10349.3–11956.1 | 8965.1–11265.3 |
| Nested(128) | 504.9–575.6 | 379.4–388.9 |
| Page | 6163.8–7090.7 | 4328.7–6006.4 |
| Bulk(8) | 8156.1–9563.2 | 6515.5–7556.0 |
| Bulk(512) | 1153.1–1293.6 | 492.4–521.1 |

Throughput divides the number of measured operations by their summed latency, including outliers. It excludes warmup, reporting and write cleanup. This is not concurrent HTTP/server throughput or end-to-end wall-clock throughput.

## Conversion, startup, and memory

CPU-only input serialization and result deserialization probes exclude fixture construction, JSON-value cloning, and SQL work. The table reports medians across three run medians.

| Workload | Input → JSON value (µs) | JSON value → typed result (µs) | Input / result serialized bytes |
|---|---:|---:|---:|
| Flat(8) | 0.08 | 3.04 | 10 / 286 |
| Flat(1024) | 0.08 | 246.27 | 13 / 42474 |
| Nested(1) | 0.17 | 3.50 | 10 / 322 |
| Nested(128) | 0.08 | 269.06 | 12 / 47378 |
| Page | 0.12 | 4.96 | 13 / 1284 |
| Bulk(8) | 2.29 | 0.04 | 370 / 2 |
| Bulk(512) | 201.25 | 0.04 | 24878 / 2 |

These probes show part of the JSON bridge cost; they exclude row-to-JSON conversion, response shaping, validation, planning, dispatch, and engine-side nested-array parsing. They are not an additive decomposition of the latency difference. The embedded bridge uses JSON values in memory, without a text JSON/network round-trip.

First schema discovery/parse/check/lowering took 0.557–0.886 ms per process. This excludes Rust compilation, pool setup, and seeding, and benefits from warm OS filesystem caches. [Consumer build costs](compile-time.md) are measured separately.

External `/usr/bin/time -l` reported 12.30–12.33 MiB whole-process peak RSS. That combines both paths, fixture, caches, runtime, allocator, and reporting. There are no allocation counts or per-path memory measurements; serialized byte sizes are not heap usage.

## Interpretation and limits

Larger materialized reads and writes show meaningful overhead in this environment. The measurements justify investigating conversion/planning costs when profiling an actual application, without prescribing a performance rewrite. Small absolute differences overlap scheduler noise. SQLite in memory omits network latency, server contention, shards, TLS, guards, idempotency, streaming, decimals, and HTTP. Different payloads, concurrency levels, drivers, and production hardware can change the balance.

Executed SQL and `EXPLAIN QUERY PLAN` details are included with bound values in the raw report. Flat reads use primary-key range scans; nested reads use the owner primary key and item owner index with correlated aggregation and a temporary sort tree; the page/count scan item. Bulk plans show VALUES-clause scans of 8, 300, and 212 rows, matching the chunk boundaries. CI checks artifact freshness, statement/result/data parity and compilation, with **no wall-clock performance threshold**.

Reproduce with the commands and measurement boundaries in the [harness guide](../benchmarks/embedded-runtime/README.md). Publish repeated measurements and variance for any future performance claim.
