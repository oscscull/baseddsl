# Performance

The embedded client executes through the engine and converts typed values to and
from JSON values in memory. It avoids HTTP but still pays engine and conversion
costs. Profile your application's workload before choosing it.

A matched in-memory SQLite measurement on an Apple M4 Max found larger flat reads,
nested reads, and bulk writes about 1.85×, 1.40×, and 2.40× the direct-SQLx latency.
The two paths used the same SQL, pool, and final Rust types. Small-operation results
varied across runs. These figures do not predict networked databases or concurrent
HTTP throughput. Raw [measurements](../../benchmarks/embedded-runtime/measurements/apple-m4-max.json)
retain the environment, statement counts, and query plans.

To rerun the Rust harness:

```sh
make ci-runtime-benchmark
cargo run --release --locked --manifest-path benchmarks/embedded-runtime/Cargo.toml -- 200
```

Repeat in fresh processes and compare variance. Compilation is outside the runtime
timing. The harness checks result/write parity before sampling. Whole-process RSS
combines both paths; it is not a per-path allocation measurement.
